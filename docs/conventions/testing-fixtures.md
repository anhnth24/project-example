# Test pyramid, fixtures and evidence

Test type follows the failure boundary, not the team owning the code.

| Layer | Scope | Location | Required command/evidence |
|---|---|---|---|
| Unit | pure function/state transition | crate/package beside code | `cargo test` / `vitest run` |
| Integration | repository, adapter, native process, service boundary | crate `tests/`, Compose test | isolated dependency + cleanup |
| Contract | Rust↔JSON↔TypeScript/OpenAPI/SSE | `openapi/fixtures`, generated snapshots | schema + round-trip |
| Denial | tenant/ACL/auth/upload abuse | server security tests | explicit forbidden outcome |
| E2E | browser/API/worker vertical slice | `tests/e2e` | deployed commit/environment |
| Benchmark/golden | quality/performance regression | `bench/markhand_web` | workload + environment fingerprint |
| Migration | fresh/upgrade/mixed-version/rollback compatibility | server migration tests | source/target versions + checksums |
| Restore/chaos | backup/recovery/lease/process failure | runbook/drill artifacts | RPO/RTO and recovery evidence |

## Fixture contract

Every committed fixture is declared in `tests/fixtures/manifest.json` with:

- globally unique stable `id`;
- repository-relative `path`, never absolute or `..`;
- SHA-256 checksum;
- `kind`, owner and deterministic generation/source;
- license/provenance and `sensitive: false`;
- optional fixed time/IDs needed for replay.

Fixtures must be synthetic, public, or demonstrably de-identified. Customer documents,
credentials, hostnames, tokens and production exports are forbidden.

Run:

```bash
python3 scripts/check-fixtures.py
python3 scripts/check-fixtures.py --self-test
```

## Determinism

- Pin IDs, timestamps, random seed, locale and timezone.
- Normalize output before golden comparison only when normalization is part of the
  product contract; do not normalize away real regressions.
- Regenerate fixture through a documented command and review checksum changes.

## Evidence and CI retention

- Small textual snapshots/manifest stay in Git.
- Raw corpus, model binaries, database dumps and large benchmark outputs use CI
  artifacts/object storage with checksum, retention and access policy.
- Report records commit, command, environment, fixture manifest hash, parameters and
  result. A screenshot alone is not machine-verifiable evidence.
- Default CI artifacts contain no document content, prompt, PII, token, key or signed
  URL. Secret canary scan runs before upload.

## Migration evidence

Record fresh apply, supported-version upgrade, mixed-version compatibility and forward
rollback. Include migration manifest hash, row counts, lock duration and failure
disposition; never attach a production database dump.
