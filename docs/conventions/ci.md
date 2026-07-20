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
- The Rust job selects the smallest crate scope from the changed paths:
  - `server` PRs run `knowledge,server` **lib tests** (~2–3 min after cache warm).
  - Workspace Clippy baseline runs only on the **full** Rust gate (`Cargo.lock`, CI
    Makefile/classifier changes).
  - Integration test binaries and the full workspace test matrix run on the full gate
    and on `master` pushes that touch the matching paths.
- Spike report/validator edits are checked in `changes-and-static` only; they no
  longer trigger the heavy `dev-stack` job by themselves.
- `dev-stack` reuses `rust-cache` so `dev-server-smoke` can hit the same `target/`
  cache as the Rust job on subsequent runs.
- Linux bundle smoke (including native-runtime preparation) runs only for
  packaging/runtime configuration changes; the full Linux/Windows/macOS installer
  matrix remains release-only.
- Phase 0 corpus changes run a dedicated Python job that installs the pinned generator
  requirements, regenerates artifacts and enforces strict dual-review adjudication.
- A CI/Makefile/classifier/toolchain change deliberately activates every group.
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
