# Third-party notices

## Telegram Database Library (TDLib)

The schema-derived constructor catalog in `src/generated/schema_ids.rs` is generated from these files at TDLib commit `a17f87c4cff7b90b278d12b91ba0614383aaee82`:

- `td/generate/scheme/telegram_api.tl`
- `td/generate/scheme/mtproto_api.tl`
- `td/generate/scheme/td_api.tl` (audit baseline only)

Upstream: <https://github.com/tdlib/td>

TDLib is distributed under the Boost Software License 1.0. A copy is included in `LICENSE-TDLIB-BOOST-1.0.txt`.

The generated catalog contains protocol declaration names, constructor identifiers, declaration kinds, and result type names. No GPL client implementation source is copied into this crate.

The password SRP behavior in `src/srp.rs` was independently implemented from
the public protocol and the Boost-licensed `td/telegram/PasswordManager.cpp` at
the same pinned TDLib commit. No GPL client password implementation is copied.
