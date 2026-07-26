# Phase 1B O-chain live gate run — all six gates pass

Date: 2026-07-26
Commit: `f4f33cd1b476e07d69594dec269002f1159f1b70`
Compose project: `markhand-poc-f02-20260726t121843z-1815269-17292`
Host: 24-core Ubuntu 22.04, Docker limited to 10 CPU, 60 GB free disk
Embedding: `mock` profile (8 dimensions, deterministic)

## Result

| Gate | Status | Report sha256 prefix |
|---|---|---|
| P1B-F02 clean boot | pass (81 checks, 0 fails) | `9d7214df30e57a95` |
| P1B-O01 telemetry/audit | pass | `e8efc7b6975fdb4b` |
| P1B-O02 alerts/runbooks | pass | `56f0475a26fd174d` |
| P1B-O04 release suite | pass | `949e14202849cf8b` |
| P1B-O03 blue/green restore | pass | `66b5045a80925f90` |
| P1B-O05 soak | pass | `a1a6d0e6ee57df4d` |

Every gate ran against the same commit, the same Compose project, the same image
ids and the same index signature, in the order F02 → seed → O01 → O02 → O04 →
O03 → O05, with each gate re-reading the project from the F02 evidence.

## O05 measurements

Duration exactly 1800 seconds, failure injection enabled, O03 restore invoked in
the same run.

| Metric | Measured | Threshold |
|---|---:|---:|
| Ingest throughput | 356 documents/hour | >= 300 |
| Query p95 | 302 ms | <= 500 |
| Query p99 | 418 ms | <= 1000 |
| RSS growth | 15.6 MB | <= 256 |
| Temp growth | 0.06 MB | <= 512 |
| Queue depth | 0 | <= 100 |
| DB connections | 19 | <= 40 |
| Resource coverage | 362 of 362 samples, 7.5s max gap | >= 325, <= 12.5s |
| Request errors outside injection | 0 | 0 |

Completions: 178 of 180 ingests, 3566 of 3600 queries, 89 of 90 deletes, 4 of 5
reconciles. The 38 recorded errors all fall inside injection windows under both
attributions the report carries — by overlap with the operation's lifetime and
by the instant the outcome was recorded — so the pass does not depend on the
attribution change made during this investigation. They are 34 query 503s, 2
upload 503s and 1 delete 503 during the dependency blip, plus one reconcile
enqueue the blip refused.

Recovery: two worker kills and one dependency blip, all recovered, observed
equals expected. Post-restore retrieval passed against the attested green
endpoint: retained documents still answer with citations, deleted ones do not,
and a low-privilege client is refused.

## Product defects this gate run found

The soak is the only test that exercises the whole stack at once for half an
hour, and each of these was invisible to every other gate.

- **No consumer for `delete` or document-drift `reconcile` jobs.** The POC ran
  convert, index and embedding workers only. On the target host 45 delete jobs
  and 3 reconcile jobs sat pending with zero attempts and a 55-minute-old
  `available_at`. Deleted content was never reclaimed from object storage or the
  vector index, and the O03 consistency backup — which fences writes and waits
  for the queue to empty — could never proceed. Fixed by running both workers.
- **A long-running reconcile worker in dry-run mode never terminates a job.**
  Dry-run releases the job back to pending to preserve repair intent, which is
  right for the operator-invoked one-shot and pathological for a service: one
  job cycled leased-to-pending every two seconds indefinitely. The service now
  runs in repair mode; the one-shot keeps dry-run.
- **The restore drill's grounded-query proof only worked on a near-empty
  collection.** It picks whichever document indexed last, then asks a fixed
  question unrelated to it. After 1800 seconds of load the newest document is
  one of the soak's uploads, so a healthy restore looked like a failed one. The
  question now comes from the canary document itself.
- **Convert sandbox confinement on AppArmor hosts.** The nested sandbox needs
  `mount`, which `docker-default` denies; the worker crash-looped. Added a
  `markhand-convert` profile and host detection. Separately, the converter's
  1 GiB address-space limit is too small on many-core hosts because the rayon
  pool scales with CPU count; raised to 4 GiB.

## Harness defects fixed

- Each upload now carries a unique marker. One marker per format meant the
  document under test competed with the whole collection for a top-N slot, and
  with the 8-dimension mock embedding every document matches every query.
- PNG markers are built from words rather than random characters. Hex markers
  round-tripped through OCR at 10 of 12 on this host (0 read as O, 1 as l), and
  restricting the alphabet made it worse at 23 of 30 because Tesseract inserts
  glyphs into strings it cannot read as words. Words: 30 of 30.
- The client re-authenticates on 401. The access token lives 900 seconds and the
  run lasts 1800, which silently failed half of every earlier run.
- The sampler collects concurrently and holds a fixed cadence, rather than
  sleeping a full interval after each collection round.
- Failure reasons are recorded per actor, which is what made the 401 and the
  retrieval-visibility diagnoses possible at all.
- Completeness no longer counts events destroyed by an injected fault against
  the minimum, and the runner refuses to read a gate status from a report the
  gate did not rewrite.

## Scope

This qualifies the single-org POC on `poc-compose` against
`G0-CAP-INGEST-THROUGHPUT-POC` (the SLA normal tier). It makes no Profile B
capacity claim: the peak gate of 1200 documents/hour belongs to
`on-prem-reference`, and this stack caps each worker at 1 CPU and serves
embeddings from a mock. Retrieval quality is not measured here either — the
8-dimension mock cannot discriminate, so the `G0-RET-*` gates remain the only
evidence for answer quality.

The container caps reserve roughly 8.5 CPU. An 8-vCPU host measured 168
documents/hour on the same code and should be expected to fail.
