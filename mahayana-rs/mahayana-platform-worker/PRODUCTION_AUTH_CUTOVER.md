# Production Rust account issuer hard cut-over

This is a one-way production cut-over. The Rust Worker accepts and issues only
RS256 Mahayana account tokens. It does not verify HS256 tokens, return the old
`token` alias, proxy to the JavaScript Worker, or synchronize credentials from
Flutter.

The account session contract is:

- RS256 access token: 15 minutes, `iss=https://api.ombhrum.com`,
  `aud=mahayana-platform`, `token_use=access`;
- opaque rotating refresh token: fixed 30-day session lifetime;
- access and refresh credentials remain in the Rust keyring/runtime and never
  cross the Flutter or Mini App bridge;
- refresh-token reuse revokes the complete session family;
- successful PBKDF2 authentication writes an Argon2id sidecar credential. The
  old PBKDF2 columns are read only and are not updated by Rust.

## Required Cloudflare resources

The production Worker must have all bindings in
`wrangler.auth-production.toml.example`: `ACCOUNT_DB`, `PLATFORM_DB`,
`PLUGIN_PACKAGES`, and `PLATFORM_EVENTS`. A custom-domain deployment is not an
auth-only overlay: once attached to `api.ombhrum.com`, every route is served by
Rust and an unknown route returns 404.

Generate a dedicated RSA-3072 key pair outside the repository and install these
values with `wrangler secret put`:

- `ACCESS_TOKEN_PRIVATE_KEY_PEM`;
- `ACCESS_TOKEN_PUBLIC_KEY_PEM`;
- `ACCESS_TOKEN_JWKS`.

`ACCESS_TOKEN_KEY_ID` must equal the JWKS `kid`. There is deliberately no
`LEGACY_HS256_SECRET` or legacy Worker service binding.

## Mandatory gates

1. Export the production `fabushi-db`, calculate SHA-256, and keep the export
   until the cut-over has completed its observation period. If Cloudflare's
   asynchronous export remains stuck, record a successful official D1 Time
   Travel bookmark immediately before the additive migration and verify that
   `wrangler d1 time-travel info fabushi-db` can read it back.
2. Apply `account-migrations/0001_account_auth.sql` only to `ACCOUNT_DB`.
3. Apply `migrations/0001_platform.sql` only to `PLATFORM_DB`.
4. Verify every currently used production API path exists in the Rust router.
   The custom domain must not be moved while any shipped Flutter flow depends
   on an unported JavaScript route.
5. `rg` must find no Flutter persistence, logging, parsing, or HTTP attachment
   of Mahayana bearer/refresh credentials. Flutter may pass only the boolean
   `authenticated` intent to the Rust command ABI.
6. Build and test native Rust, the Worker WASM target, and the browser
   `mahayana-web` WASM target. Rebuild the checked-in wasm-bindgen output.
7. Deploy to a `workers.dev` staging URL first. Test a real production-compatible
   account through login, user-info, two refresh rotations, reuse rejection,
   logout, and post-logout rejection.
8. Test marketplace, wallet, purchases, model usage reservation/capture, and
   every Flutter API route against the staging Worker.
9. Add Cloudflare source-IP rate limits for `/api/auth/login` and
   `/api/auth/refresh`. Rust additionally limits ten known-account failures in
   a rolling 15-minute window.
10. Only after all gates pass, deploy the same artifact with the
   `api.ombhrum.com` custom domain.

## 2026-07-18 deployment state

- The Rust Worker is deployed only to
  `https://mahayana-platform.bhrumom.workers.dev`.
- Health, RS256 JWKS, unknown-route 404 behavior, and an invalid-credential
  login against the production-compatible account schema have been checked.
- The production account database has the additive Rust auth migration. The
  pre-migration D1 Time Travel bookmark is recorded in the deployment log; no
  existing `users` password field was rewritten.
- The new platform D1 schema, plugin R2 bucket, event Queue, and Worker signing
  secrets are provisioned.
- The old Fabushi router currently exposes about 115 static API paths while
  this Worker exposes 17 routes. Moving the whole custom domain now would
  create production 404s. The domain remains attached to the existing Worker
  until every shipped Flutter path is either implemented in Rust or explicitly
  retired.

This is intentionally not a legacy proxy. The staging Worker returns 404 for
unknown paths and accepts no HS256 token. The remaining production-domain gate
is route coverage, not a compatibility fallback.

## D1 invariant implementation

Cloudflare D1's remote query endpoint rejects SQLite `CREATE TRIGGER` bodies
with `incomplete input`, although the same schema is valid in local SQLite.
The production migration therefore uses ordinary tables, CHECK constraints,
unique keys and views. The Rust Worker enforces journal balance and AI usage
capacity with ordered D1 batch transactions, conditional inserts, event-gated
capture, and terminal-state predicates. There is no client-writable journal or
usage mutation route.

## Production checks

The login response must contain `accessToken`, `refreshToken`,
`accessTokenExpiresAt`, `refreshTokenExpiresAt`, `sessionId`, `deviceId`, and
`user`. It must not contain `token`. Old clients and old HS256 sessions are
expected to stop working and must reauthenticate through a newly shipped Rust
client.

Observe login and refresh outcomes, 401/404 rates, session revocations,
refresh-reuse events, route latency, model usage reservations, and ledger
balance invariants for at least one access-token lifetime.

## Recovery

Do not reconnect the JavaScript signer and do not re-enable HS256. Recovery is
a forward Rust deployment. If the additive D1 migration itself is defective,
detach the domain, repair the Rust Worker, and use the verified export only for
database recovery. Never overwrite the existing `users` password columns with
sidecar data.
