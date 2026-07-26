# Phase 1B live gate run — 2026-07-25

Commit under test: `302b6e44c870924c880f1144eac781bd2d303bb0` (clean tree).
Compose project: `markhand-poc-f02-20260725t053206z-39674-24731`.
Host: Windows 11 + WSL2 Ubuntu 22.04, Docker Desktop 28.3.2 / Compose 2.38.2,
8 vCPU and 10 GB allocated to WSL2 out of 22 vCPU / 31.5 GB on the machine.
Index signature `72dda200…2978874`, POC images `markhand-api:poc` /
`markhand-worker:poc`, embedding profile `mock` plus the `telemetry` profile.

Every gate below ran against the same live stack, in the order
F02 → O01 → O02 → O04 → O03 → O05. Machine reports are written outside the
source tree (`.artifacts/markhand_web/…`, gitignored) as the release-gate
runbooks prescribe, so the evidence files committed under
`bench/markhand_web/reports/phase-1b-gate/` still describe the earlier Cloud VM
run, not this one.

## Results

| Gate | Status | Key measurement | Report sha256 (prefix) |
|---|---|---|---|
| P1B-F02 boot/isolation | pass | 67 checks, 0 fails, clean project boot measured | `0064435387c8fef1` |
| P1B-O01 telemetry/audit | pass | 16/16 async canary proofs, 0 blockers | `0a9becb0af977e12` |
| P1B-O02 alerts/runbooks | pass | 31 passes, live fire 150s → resolve 24s | `737c30f5dab4bbef` |
| P1B-O03 backup/restore | pass | RPO 26s, query-ready RTO 34s, full-vector RTO 34s | `b92ecd8c5fd4b6ec` |
| P1B-O04 release suite | pass | 8/8 formats, release gate 3/3 | `f4cdb03a185c9da8` |
| P1B-O05 soak | **fail** | 1800s executed; throughput/latency gates missed | `22753d3bc410eed9` |

## O05 detail

Prerequisites validated (`canonicalBinding=pass`) and the run lasted exactly
1800 seconds. Passing gates: `recovery` (two worker kills and one dependency
blip, all recovered, expected==observed), `rssGrowth` 37.4 MB, `tempGrowth`
0.02 MB, `queueDepth` 0, `dbConnections` 16, `unboundedGrowth`.

Failing gates and the numbers behind them:

- `ingestThroughput` 168 docs/hour — 84 of 900 uploads reached terminal indexed
  state inside the bounded timeout (gate needs ≥ 1200/hour).
- `queryP99` 9277 ms (limit 1000) and `queryP95` 253 ms; 1902 of 3600 queries
  returned the expected marker with citation.
- `requestErrors` 2553, of which only 6 fell inside an injection window.
- `resourceCoverage` 129 of 325 required samples with a 16.1s maximum gap
  (12.5s allowed) — the sampler thread was starved by CPU contention.
- `completeness`, `workloadDrain` and `reconcile` fail as consequences of the
  same shortfall.
- `postRestoreRetrieval` is `unknown`: the in-run O03 restore refused with
  `STRICT_DRAIN_FAILED` (56 jobs still in flight), so no attested green endpoint
  existed to probe.

Root cause is host capacity, not a defect in the stack. The POC services alone
reserve roughly 7.5 of the 8 vCPUs Docker is allowed, so the convert/index
workers, the API and the Python driver contend for the same cores. A 30-second
smoke against the identical stack measured query p95 69 ms, p99 77 ms and 1800
docs/hour, so the canonical profile is reachable with more CPU. Re-run the
official soak after raising the WSL2 allocation in `.wslconfig` (or on a Linux
runner) before claiming POC qualification.

## Environment work required to reach these results

These were environment gaps, not product changes:

1. The POC seed deliberately carries no `password_hash`, so the HTTP harnesses
   got 401 until the admin password was seeded with `dev-hash-password`.
2. O04's cross-tenant probe needs real foreign-org resources; a second org with
   its own collection and document was seeded into POC Postgres.
3. Integration suites create and drop their own buckets, so they need the MinIO
   root identity rather than the bucket-scoped application key, plus
   `MARKHAND_TEST_QDRANT_ADMIN_API_KEY`.
4. The convert sandbox only adds tessdata to its Landlock allowlist when an
   absolute path is exported. The worker image sets `TESSDATA_PREFIX` itself;
   host-side suites must export `FILECONV_TESSDATA` or PNG OCR fails with
   "Could not initialize tesseract".
5. The worktree is a Git-for-Windows checkout with `core.autocrlf=true`, so
   Linux-side git inside WSL sees every tracked file as modified. Gate runs need
   the same normalization (`GIT_CONFIG_KEY_0=core.autocrlf`), and the text-like
   soak fixtures must be restored to HEAD bytes before preflight regenerates
   them.
6. F02 must be collected with the final environment already in place. An earlier
   attempt enabled the telemetry profile after the boot evidence, which
   recreated the API and worker containers and invalidated O04's F02 binding.

## Code change in this run

`bench/markhand_web/scripts/run_o04_release_suite.py` — `--validate-report` now
resolves the raw directory against the report's own evidence root
(`O04_OUTPUT_DIR`, else the report directory) instead of the in-tree default.
Before this, out-of-tree evidence (what `--release-gate` and CI produce) always
failed with `raw_dir_outside_evidence_root`, while in-tree evidence tripped
`git_dirty`, so no O04 run could validate. Path traversal, absolute paths and
missing directories are still rejected, and a regression test covers both the
accepted out-of-tree layout and the rejected `../escape` case.

## Reproduction

Drivers used for this run live in `.artifacts/p1b/` (gitignored) and wrap the
documented commands: `deploy/scripts/poc-boot-evidence.sh`,
`bench/markhand_web/scripts/run_o01_telemetry_evidence.py`,
`deploy/scripts/o02-alert-tabletop.sh`, `deploy/scripts/o04-release-suite.sh
--release-gate`, `deploy/scripts/o03-bluegreen-restore-drill.sh`, and
`deploy/scripts/o05-soak.sh --enable-failure-injection --invoke-o03-restore`.
