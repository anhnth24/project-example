# Quality tooling and CI

The root `Makefile` is the command authority for local and CI quality gates. CI must
call the same target a contributor runs locally; workflow YAML only provisions native
tools/caches and parallelizes targets.

## Core commands

```bash
make install
make check-toolchain
make check-static
make check-rust
make check-rust-tests
make check-web
make check-desktop
make check-foundation
```

Local services use `make dev-up`, `dev-health`, `dev-down`, `dev-reset`. Linux bundle
validation uses `make bundle-linux`.

## Dependency and supply-chain baseline

- One root `pnpm-lock.yaml`; package-level lockfiles are forbidden.
- Cargo and pnpm install in locked/frozen mode.
- Cargo git dependencies are denied by default; path dependencies cannot escape repo.
- External Cargo dependencies require license metadata.
- Compose images use immutable version tags, never `latest`.
- GitHub Actions are pinned to full commit SHA with human-readable version comments.
- Dependency/native updates require source/version/license review and relevant smoke
  evidence; model IDs/binaries and customer corpus remain outside Git.

`python3 scripts/check-dependency-policy.py` enforces this baseline.

## CI behavior

- Every PR and `master` push runs the consolidated static/foundation gate.
- Heavy Rust, desktop frontend, web and dev-stack jobs run only when their owned paths
  change, on both PR and `master`. This keeps direct pushes safe without running
  unrelated product gates.
- The Rust job runs `scripts/run-rust-ci-fast.sh` (fmt + clippy + tests in one step):
  - **smoke** (CI/Makefile/Rust-script edits): server **lib tests** only (~1–2 min).
  - **scoped** (`server`, `core`, …): smallest crate set; server PRs skip duplicate
    knowledge compile (~2–3 min after cache warm).
  - **workspace** (`Cargo.lock`, root manifests): all crates except desktop, no GTK
    (~3–4 min).
  - **full** (desktop paths): entire matrix including `fileconv-desktop`.
  - Clippy uses `--lib` on PR gates; `--all-targets` on full/master integration.
  - Integration test binaries run on `master` push (`RUST_INTEGRATION=true`) and the
    full desktop gate.
- A Makefile or Rust-script change activates **rust + toolchain** only, not frontend,
  web, corpus, bundle, or dev-stack.
- Spike report/validator edits are checked in `changes-and-static` only; they no
  longer trigger the heavy `dev-stack` job by themselves.
- `dev-stack` uses tiered profiles via `deploy/scripts/dev-stack-ci.sh`:
  - **lite** (`deploy/scripts/**`): compose config + `dev-up`/`dev-health` only.
  - **full** (`deploy/dev/**`, spike compose): adds spike lifecycle and `check-spike`.
  - Skips `dev-server-smoke` when the Rust job already validated `fileconv-server`.
- `dev-stack` keeps `rust-cache` only when it may run `dev-server-smoke` without a
  parallel Rust job.
- Linux bundle smoke (including native-runtime preparation) runs only for
  packaging/runtime configuration changes; the full Linux/Windows/macOS installer
  matrix remains release-only.
- Phase 0 corpus changes run a dedicated Python job that installs the pinned generator
  requirements, regenerates artifacts and enforces strict dual-review adjudication.
- A CI workflow or classifier change deliberately activates every group; Makefile/Rust
  script edits activate rust + toolchain only.
- A new commit on the same PR cancels the older in-progress run. `master` runs are not
  grouped or canceled because each run classifies a different push delta.
- Installer matrices run only for `markhand-v*` tags or manual dispatch, never for an
  ordinary `master` push.
- The issue-sync workflow remains path-filtered to backlog/sync changes.
- Caches may speed work but a clean cache miss must pass.
- CI permissions remain read-only except dedicated issue-sync/release workflows.
- Artifacts follow [`testing-fixtures.md`](testing-fixtures.md): no secret/PII/content
  leakage, explicit retention and checksums.

## Failure handling

Failures must name the command and recovery action. Do not mute lint/test failures or
expand baselines without justification. Intentional negative fixtures live inside each
validator's `--self-test`, so CI proves denials as well as happy paths.
