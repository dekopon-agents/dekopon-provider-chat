#!/usr/bin/env python3
"""Offline startup witnesses using brokerd 0.47.0 and a synthetic bearer token."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests" / "fixtures"
BROKER = os.environ.get("DEKOPON_BROKERD", "dekopon-brokerd")


def command(*argv):
    return subprocess.run([BROKER, *map(str, argv)], capture_output=True, text=True, timeout=25)


def boot(config, ready=False):
    with subprocess.Popen([BROKER, "--config", str(config)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as proc:
        if ready:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline and proc.poll() is None and not (config.parent / "broker.sock").exists():
                time.sleep(0.1)
            assert (config.parent / "broker.sock").exists(), proc.communicate(timeout=5)
            proc.terminate()
        stdout, stderr = proc.communicate(timeout=25)
        return proc.returncode, stdout + stderr


def fixture():
    temporary = tempfile.TemporaryDirectory(prefix="chat-broker-", dir="/tmp")
    path = pathlib.Path(temporary.name)
    os.chmod(path, 0o700)
    for name in ("broker.yaml", "broker-credentials.yaml", "policies.cedar"):
        shutil.copyfile(FIXTURE / name, path / name)
        os.chmod(path / name, 0o600)
    shutil.copyfile(ROOT / "chat-provider.wasm", path / "chat-provider.wasm")
    config = path / "broker.yaml"
    config.write_text(config.read_text().replace("TEST_UID", str(os.getuid())).replace("SOCKET_PATH", str(path / "broker.sock")))
    os.chmod(config, 0o600)
    return temporary, path


def main():
    assert command("--version").stdout.strip() == "dekopon-brokerd 0.47.0"
    for defect, expected_check, expected_boot in (
        (None, None, None),
        ("unknown slot", "conversation.unknown", "conversation.unknown"),
        ("metadata mismatch", "effect", "effect"),
        ("credential destination", "credential slack-history is selected", "outside credential"),
    ):
        temporary, path = fixture()
        try:
            config = path / "broker.yaml"
            if defect == "unknown slot":
                config.write_text(config.read_text().replace("conversation.thread", "conversation.unknown"))
            if defect == "metadata mismatch":
                config.write_text(config.read_text().replace("chat.slack.history.read:\n", "chat.slack.history.read:\n        effect: external-write\n"))
            if defect == "credential destination":
                credential = path / "broker-credentials.yaml"
                credential.write_text(credential.read_text().replace("destinations: [slack.com]", "destinations: [example.com]"))
            checked = command("check", config)
            check_text = checked.stdout + checked.stderr
            if defect in (None, "credential destination"):
                assert checked.returncode == 0, (defect, check_text)
            else:
                assert checked.returncode != 0, (defect, check_text)
            if expected_check:
                assert expected_check in check_text, (defect, check_text)
            code, output = boot(config, ready=defect is None)
            if defect is None:
                assert "broker_started" in output, output
            else:
                assert code != 0 and expected_boot in output, (defect, code, output)
                if defect == "credential destination":
                    assert "slack.com" in output and "slack-history" in output, output
            print(f"{defect or 'good config'}: check={checked.returncode}, boot={code}")
        finally:
            temporary.cleanup()


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
        print(f"broker fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
