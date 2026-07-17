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

## Evidence

Record OS/tool versions, commands and final commit. Do not upload secret-bearing logs
or corpus. CI green is necessary but deployment/benchmark issues still require their
explicit evidence before `Done`.
