# Guidance for coding agents

The Chat provider for Dekopon is one Wasm component with platform-specific, read-only history capabilities. [README.md](README.md) describes its interface.

## Contract

- This file and its linked documents are the repository contract, including for a cloud agent in a bare checkout.
- Core's [boundaries](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#boundaries-that-must-survive), [proportionate remedies](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#proportionate-remedies), [Rust guidelines](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#rust-guidelines) and [comments](https://github.com/dekopon-agents/dekopon/blob/main/AGENTS.md#comments) apply.
- Tags, releases and version bumps need owner authorization. PRs never push `main`.

## Chat boundaries

- Only broker-attested conversation slots select a channel/thread. No model-facing channel, thread, workspace or URL input; no generic HTTP passthrough or history fallback.
- Slack reads only `conversations.replies` at the claimed thread; Discord reads only the attested API channel. Each capability has its own credential binding and GET request template. No write capability or provider asset fetch.
- The guest never sets `authorization`. The broker injects destination-bound credentials after request validation. Do not expose tokens, private response headers, URLs or platform error bodies in outputs.
- A history attachment is metadata only (`available:false`); it cannot be fetched by this provider. Bound messages, pages, decoded response bytes and output text. A failed page is not an empty success.

## Build and verify

The shared [provider-workflows](https://github.com/dekopon-agents/provider-workflows) CI runs `ci / validate`. Clone it as a sibling if absent. Run these steps before pushing:

```console
cargo fmt --all --check
cargo deny --all-features check bans licenses sources advisories
cargo clippy -p dekopon-chat-provider --all-targets --locked -- -D warnings
cargo clippy -p dekopon-chat-provider --lib --target wasm32-unknown-unknown --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p dekopon-chat-provider --no-deps --locked
if test -x ../provider-workflows/build.sh; then ../provider-workflows/build.sh; else ../../provider-workflows/build.sh; fi
DEKOPON_PROVIDER_COMPONENT="$PWD/chat-provider.wasm" cargo test -p dekopon-chat-provider --locked
python3 tests/verify_broker.py
```

`build.sh` uses this worktree's target directory. No test contacts the Slack or Discord API. A component test must read `DEKOPON_PROVIDER_COMPONENT` with `expect`, without fallback or skip; use the published testkit's `Harness` and `Native::requests()` to check the full request URI, request count and absence of guest credentials. Native-only tests say so in their names. Test names state the invariant; typed errors are matched by variant, not by Display. Fixture credentials are synthetic, never read from local secrets.

## Change rules

- No comments or doc comments by default except when core's comments rule warrants them. Clap `///` is CLI help text.
- Exact SDK and testkit pins move together. Regenerate Cargo.lock with Cargo.
- Leave CHANGELOG.md for release prep. User-visible PR commits carry one physical `Changelog:` trailer per entry.
- No live tests, credential changes, grants, publication or rollout in a source PR.
