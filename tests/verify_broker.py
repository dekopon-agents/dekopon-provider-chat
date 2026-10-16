#!/usr/bin/env python3
"""Offline startup witnesses using brokerd 0.47.0 and a synthetic bearer token."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests" / "fixtures"
BROKER = os.environ.get("DEKOPON_BROKERD", "dekopon-brokerd")


def command(*argv):
    return subprocess.run([BROKER, *map(str, argv)], capture_output=True, text=True, timeout=25)


def boot(config, ready=False):
    proc = subprocess.Popen([BROKER, "--config", str(config)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        if ready:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline and proc.poll() is None and not (config.parent / "broker.sock").exists():
                time.sleep(0.1)
            if not (config.parent / "broker.sock").exists():
                raise AssertionError("broker did not bind socket")
            proc.terminate()
        stdout, stderr = proc.communicate(timeout=25)
        return proc.returncode, stdout + stderr
    except (subprocess.TimeoutExpired, AssertionError):
        proc.kill()
        proc.communicate(timeout=5)
        raise


def timeout_cleanup_witness():
    class TimedOutBroker:
        killed = False
        calls = 0
        returncode = -9

        def communicate(self, timeout):
            self.calls += 1
            if not self.killed:
                raise subprocess.TimeoutExpired("brokerd", timeout)
            return "", ""

        def kill(self):
            self.killed = True

    process = TimedOutBroker()
    with patch("subprocess.Popen", return_value=process):
        try:
            boot(pathlib.Path("/tmp/unused-config.yaml"))
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("timeout must propagate")
    assert process.killed and process.calls == 2, "timed-out broker must be reaped"


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
    timeout_cleanup_witness()
    assert command("--version").stdout.strip() == "dekopon-brokerd 0.47.0"
    for only in ("slack", "discord"):
        temporary, path = fixture()
        try:
            config = path / "broker.yaml"
            text = config.read_text()
            if only == "slack":
                text = text.split("      chat.discord.history.read:")[0]
            else:
                text = text[:text.index("      chat.slack.history.read:")] + text[text.index("      chat.discord.history.read:"):]
            config.write_text(text)
            policy = path / "policies.cedar"
            blocks = policy.read_text().strip().split("\n\n")
            policy.write_text(next(block for block in blocks if f'{only}-history-on-{only}' in block) + "\n")
            checked = command("check", config)
            assert checked.returncode == 0, (only, checked.stdout, checked.stderr)
            code, output = boot(config, ready=True)
            assert code == 0 and "broker_started" in output, (only, code, output)
            print(f"{only} alone: check={checked.returncode}, boot={code}")
        finally:
            temporary.cleanup()
    for defect, expected_check, expected_boot in (
        (None, None, None),
        ("unknown slot", "conversation.unknown", "conversation.unknown"),
        ("metadata mismatch", "effect", "effect"),
        ("credential destination", "credential slack-history is selected", "outside credential"),
        ("agent credential destination", "credential discord-history-agent is selected", "outside credential"),
    ):
        temporary, path = fixture()
        try:
            config = path / "broker.yaml"
            if defect == "unknown slot":
                config.write_text(config.read_text().replace("conversation.thread", "conversation.unknown"))
            if defect == "metadata mismatch":
                config.write_text(config.read_text().replace("chat.slack.history.read:\n", "chat.slack.history.read:\n        effect: external-write\n"))
            if defect in ("credential destination", "agent credential destination"):
                credential = path / "broker-credentials.yaml"
                text = credential.read_text()
                if defect == "credential destination":
                    text = text.replace("destinations: [slack.com]", "destinations: [example.com]", 1)
                else:
                    start = text.index("  - name: discord-history-agent")
                    text = text[:start] + text[start:].replace("destinations: [discord.com]", "destinations: [example.com]", 1)
                credential.write_text(text)
            checked = command("check", config)
            check_text = checked.stdout + checked.stderr
            if defect in (None, "credential destination", "agent credential destination"):
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
                if defect == "agent credential destination":
                    assert "discord.com" in output and "discord-history-agent" in output, output
            print(f"{defect or 'good config'}: check={checked.returncode}, boot={code}")
        finally:
            temporary.cleanup()


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
        print(f"broker fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
