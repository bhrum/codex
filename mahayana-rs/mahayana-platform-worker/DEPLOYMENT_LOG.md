# Mahayana platform deployment log

## 2026-07-18 Rust hard-cut staging

- Worker: `mahayana-platform`
- workers.dev URL: `https://mahayana-platform.bhrumom.workers.dev`
- deployed version: `8da2ef7a-2b18-4629-9bfc-c71e2127148d`
- account database: additive `0001_account_auth.sql` applied
- pre-migration Time Travel bookmark:
  `00002837-00000000-000050ac-06a7a4666f268847c20cc15c98fab198`
- platform database: `0001_platform.sql` applied
- R2: `mahayana-plugin-packages` created
- Queue: `mahayana-platform-events` created
- custom domain: not attached; route-coverage gate remains open

The original SQL export request was cancelled after 43 minutes of successful
poll responses without a downloadable artifact. Cloudflare Time Travel was
then queried successfully before the account migration and is the authoritative
recovery point for this cut-over attempt.
