# Configuration and secrets

`fileconv-server` uses typed configuration. Precedence is:

```text
safe defaults < MARKHAND_CONFIG_FILE JSON < MARKHAND_* environment
```

`MARKHAND_PROFILE` is `dev`, `test`, or `prod`. Dev/test default to loopback
`127.0.0.1:8787`; production requires an explicit non-loopback bind address and
non-development database URL. Invalid profile, address or production configuration
fails before server/worker work begins.

The running server additionally requires `MARKHAND_DATABASE_URL`,
`MARKHAND_QDRANT_URL`, and `MARKHAND_MINIO_URL`; `--check-config` validates these
runtime endpoints too. Production requires `sslmode=require` for PostgreSQL; the
server's Rustls connector validates the peer certificate and hostname using the
native root store. Qdrant and MinIO require HTTPS. Local development may use the
plain HTTP and PostgreSQL endpoints in `deploy/dev/.env.example`.

## Secrets

- Never commit `.env`, API keys, database credentials, token signing keys, signed URLs
  or customer hostnames. Use [`deploy/dev/.env.example`](../../deploy/dev/.env.example)
  as the non-secret template.
- Production uses orchestrator secret mounts or environment references. Do not put a
  production secret in JSON config checked into source.
- Secret values use redacted `Debug`; errors/logs state the invalid field without
  echoing its value.
- Rotate secret by deploying a new reference, verify health, then revoke old material.
  Rotation implementation/secret manager selection is Phase 4.

## Profiles

| Profile | Bind default | Credential rule |
|---|---|---|
| `dev` | loopback | non-production local values only |
| `test` | loopback | fixture/ephemeral values only |
| `prod` | explicit required | no loopback/default database credential |

Run a config-only check:

```bash
cargo run -p fileconv-server -- --check-config
cargo run -p fileconv-server --bin fileconv-worker -- --check-config
```
