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
- Live gates are **opt-in, never per-push**. `phase1b-o04-release-gate` boots the POC
  stack and rebuilds the release server image inside Docker with no layer cache between
  runners (~20 min per run), so it runs only on a manual `workflow_dispatch` or when a
  pull request carries the `run-live-gates` label. Qualifying the POC is a deliberate
  act before a phase closes; per-push CI stays static + unit + service-container
  integration. The O05 soak is not a CI job at all: it needs a full 1800-second run on
  a host that can meet the throughput gate.
- `deployed-1c-integration` inherits the same opt-in, expensive-live-gate pattern as
  `phase1b-o04-release-gate`/`owasp-baseline`: same trigger (manual `workflow_dispatch`
  or the `run-live-gates` PR label), same `deploy/scripts/poc-up.sh` +
  `poc-health.sh` boot/teardown shape. It runs the canonical Phase 1C denial
  manifest runner (`scripts/run-phase1c-denial-suite.py` with
  `MARKHAND_TEST_REQUIRED=1`) against the deployed POC stack
  (`deploy/compose.poc.yml` ports/credentials, not `deploy/dev/compose.yml`'s),
  so a pass is evidence for the "deployed environment" half of the Phase 1C
  exit gate — the `rust-integration` job already covers the CI half against the
  ephemeral dev services. It reports to two named gates in
  `bench/markhand_web/gates.yaml`:
  - **`1C-12`** — multi-org denial suite (manifest-driven connected suite;
    sanitized JSON + Markdown artifact).
  - **`1C-13`** — security/revoke/load gate (explicitly `not_run` in the
    Markdown artifact until a dedicated suite lands).

  Both carry `failureDisposition: block-phase-1c` and `environmentId: poc-compose`.
  The job uploads `manifest-run.json` and `phase1c-denial-report.md` (never raw
  cargo output). No branch protection required check is attached yet —
  informational only until a live deployed run succeeds. See
  `plans/reports/gate-run-260803-0000-markhand-web-phase1c-denial-suite-report.md`
  for the evidence template these runs fill in.
- `dev-stack` uses tiered profiles via `deploy/scripts/dev-stack-ci.sh`:
  - **lite** (`deploy/scripts/**`): compose config + `dev-up`/`dev-health` only.
  - **full** (`deploy/dev/**`, spike compose): adds spike lifecycle and `check-spike`,
    plus `deploy/scripts/web-e2e-real.sh` (real-deployment half of P2-15) run against
    the same still-up dev stack, before `dev-down`.
  - `deploy/scripts/web-e2e-real.sh` and `web/e2e-real/**` are carved out of the
    otherwise-lite `deploy/scripts/**`/`web/**` patterns and force `full` on their own
    (`scripts/classify-ci-changes.py`'s `DEV_STACK_FULL`), so editing only the real-E2E
    harness still exercises it instead of silently classifying as `lite`.
  - Skips `dev-server-smoke` when the Rust job already validated `fileconv-server`.
    `web-e2e-real.sh` is not skipped by that flag — it is new coverage (build the SPA,
    serve it from `fileconv-server`, drive it with the Playwright `real` project), not
    a duplicate of the Rust job's own tests.
- `dev-stack` runs `rust-cache` whenever the tier is `full` (its full tier always builds
  `fileconv-server` at least once, for `web-e2e-real.sh` if nothing else) and installs
  Node/pnpm + Playwright's Chromium only for that same tier, so the lite tier's cost is
  unchanged.
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

## Security scanning (P2-15 OWASP baseline)

Three gates, split by the same two-speed philosophy as the rest of this file: fast
static scans block every relevant push; the live DAST gate stays opt-in like
`phase1b-o04-release-gate` until its findings have been triaged.

- `security-deps` — **unconditional**, runs on every PR and `master` push (seconds,
  no infra). `rustsec/audit-check` runs `cargo audit` against `Cargo.lock`/the RustSec
  advisory DB; `pnpm audit --audit-level high` runs against the root `pnpm-lock.yaml`
  (covers both `web/` and `app/`). Fails on High/Critical; informational RustSec
  advisories and pnpm findings below `high` are reported but non-blocking.
- `security-image` — gated like `linux-bundle`: only when `deploy/**` changed (new
  `deploy_images` classifier output), or unconditionally on a `master` push. Builds
  `deploy/Dockerfile.server` and `deploy/Dockerfile.worker`, scans both with
  `aquasecurity/trivy-action` (`scan-type: image`), fails on High/Critical. Accepted
  risk goes in `.trivyignore` with the same owner/scope/expiry discipline as any other
  documented exception (see `docs/adr/TEMPLATE.md`'s "Exception lifecycle").
- `owasp-baseline` — **opt-in, never per-push**, same trigger as
  `phase1b-o04-release-gate` (`workflow_dispatch` or a `run-live-gates` PR label):
  boots the POC stack via `deploy/scripts/poc-up.sh` + `poc-health.sh`, then runs
  `zaproxy/action-baseline` against the API's base URL. This is the literal "OWASP
  baseline scan" P2-15 asks for — passive HTTP checks only (missing security headers,
  cookie flags, information disclosure); it does not authenticate or exercise business
  logic, so it is **not** a substitute for Phase 1C's denial/authorization suite.
  **Warning-only**: `fail_action: false` on the action plus job-level
  `continue-on-error: true` so it can never fail the workflow yet, and it is
  deliberately **not** added to branch-protection required checks. ZAP baseline's
  alert set needs one or two real runs against this app's actual POC stack before an
  alert-filter rules file can be trusted enough to promote this to a blocking gate —
  that promotion is a deliberate follow-up, not something to guess at during initial
  wiring.
- All three new Actions are pinned to a full commit SHA with a version comment, same
  as every other workflow step; `scripts/check-dependency-policy.py` enforces this.

## Failure handling

Failures must name the command and recovery action. Do not mute lint/test failures or
expand baselines without justification. Intentional negative fixtures live inside each
validator's `--self-test`, so CI proves denials as well as happy paths.
