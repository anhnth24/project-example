# Contributor setup

## Pinned prerequisites

- Rust `1.88.0` with rustfmt/clippy.
- Node.js 20+ and pnpm `10.33.3`.
- Python 3.12+, GNU Make, Bash, curl.
- C/C++ compiler, clang, cmake and native Tauri headers listed in CI.
- Docker Engine + Compose v2 for local services.
- Optional PDF/OCR/audio assets follow `CLAUDE.md`; they are not committed.

## Clean checkout

```bash
git clone <repository>
cd project-example
make check-toolchain
make install
make check-static
make check-rust
make check-rust-tests
make check-web
make check-desktop
make dev-up
make dev-health
make dev-down
```

CI runs these same Make targets in parallel and adds Linux bundle validation.

## Common failures

- `whisper-rs` native build stale: verify C/C++/cmake, then
  `cargo clean -p whisper-rs-sys`.
- pnpm mismatch/nested lock: install pnpm 10.33.3; use only root lock.
- scan PDF runtime missing: run `bench/download_pdfium.sh`; install Tesseract `vie+eng`
  or use bundled desktop runtime.
- Compose health fail: inspect `docker compose -f deploy/dev/compose.yml ps` and logs;
  scripts already retry stable PostgreSQL/Qdrant startup.
- generated roadmap/API drift: run `python3 scripts/build-roadmap.py` or
  `pnpm --filter markhand-web api:generate`, then review the diff.

## CI failure-prevention rules

Run the matching preflight before pushing a PR; do not wait for the CI static job to
discover these deterministic failures.

- Any `Cargo.toml` or workspace dependency change must include the resulting
  `Cargo.lock` update. Run `cargo metadata --locked --format-version 1`; if it asks
  to update the lockfile, regenerate and commit it before pushing.
- Do not change files fingerprinted by `scripts/validate_spike.py` without regenerating
  the measured spike report on a real Compose run. Prefer a dedicated dev-only script
  when a server-only bootstrap does not alter spike behavior.
- The corpus job is pinned to the generator environment lock. Keep its runner image
  aligned with `bench/markhand_web/generator-environment.lock.json`; do not update a
  font checksum from a different OS.
- Server smoke must build first and apply its readiness timeout only after the binary
  starts. A cold Rust build is not a failed readiness probe.
- Configuration tests that assert a specific validation error must supply valid values
  for earlier invariants (for example, production auth settings before a bind-address
  assertion), so new fail-fast checks do not invalidate the test's intent.
- Every Rust edit must pass `cargo fmt --all -- --check` before push. This is the first
  command in the Rust CI quality gate and prevents an otherwise avoidable full rerun.

## Evidence

Record OS/tool versions, commands and final commit. Do not upload secret-bearing logs
or corpus. CI green is necessary but deployment/benchmark issues still require their
explicit evidence before `Done`.
