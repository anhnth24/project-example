# OpenAPI contract

`openapi.yaml` is the source of truth for Markhand Web public HTTP/SSE contracts.
Rust drift checks live in `fileconv_server::api::openapi` (`WIRED_API_V1_OPERATIONS`).

Regenerate the checked-in YAML after editing `generate_openapi.py`:

```bash
python3 crates/server/openapi/generate_openapi.py
pnpm --filter markhand-web api:generate
pnpm --filter markhand-web api:check
cargo test -p fileconv-server --test openapi_rate_health
```

Fixtures must round-trip through `fileconv-server::api` types. The document covers
wired R02/R04/R05 routes plus R06 health/security/SSE/error envelopes and must not
expose secrets or internal object keys.
