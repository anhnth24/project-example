# fileconv-server

Future Markhand Web API and worker boundary. Phase F contains only compileable binaries
for help/config validation; it intentionally has no HTTP framework, database, auth,
route or job implementation.

Later code follows `route → service → repository/adapter`; business operations require
an explicit `OrgContext`.

Run `cargo run -p fileconv-server -- --check-config` to validate typed configuration
without starting a listener. See [`docs/conventions/config-secrets.md`](../../docs/conventions/config-secrets.md).
