# Mahayana Runtime

This directory is the product-owned Rust layer built on top of the upstream
Codex sources in `../codex-rs`. Keeping it in a separate workspace makes
selective upstream synchronization reviewable: upstream code stays in
`codex-rs`, while Mahayana conversation routing, product authentication,
Telegram, MiniApp, FFI, WASM, CLI, and TUI code lives here.

The runtime contract is conversation-first. Every surface sends the same
commands and receives the same events whether the selected peer is the Codex
agent, a Telegram contact, a Mahayana friend, or a MiniApp.

Build profiles are intentionally explicit:

- `desktop-full`: native filesystem/process/Git plus all providers.
- `mobile-embedded`: in-process agent with app-sandbox tools.
- `web-wasm`: browser-local runtime, storage, and Web Worker transport.

No profile is allowed to silently switch to a remote Agent gateway. A remote
model endpoint is a model provider, not an Agent runtime, and must be visible
in runtime status.
