# Upstream synchronization

The Git remotes for this checkout are expected to be:

```text
origin   https://github.com/bhrum/codex.git
upstream https://github.com/openai/codex.git
```

Mahayana work branches from a reviewed `upstream/main` commit. Product code
belongs under `mahayana-rs/`; modifications below `codex-rs/` require a short
compatibility note in the commit message and a focused upstream-diff review.

Current reviewed base:

```text
repository https://github.com/openai/codex.git
branch     main
commit     2f7d89b1419bf7064346855b0acde23514b1ebc5
reviewed   2026-07-13
```

Recommended update flow:

```bash
mahayana-rs/scripts/merge-upstream.sh
cargo test --manifest-path mahayana-rs/Cargo.toml
```

The machine-readable pin is `mahayana-rs/UPSTREAM.lock`. Update it only after
reviewing the `upstream/main...HEAD` diff and running the focused Runtime test
matrix. Scheduled CI reports drift; it does not silently merge upstream code.
