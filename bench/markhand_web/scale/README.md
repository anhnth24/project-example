# P0-07 scale topology harness

This directory contains the P0-07 offline topology comparison harness for the
Markhand Web Phase 1B POC decision.

## What it does

`bench/markhand_web/scripts/run_scale_topology.py` is a stdlib-only,
in-process harness. It does not require Docker, Postgres, Qdrant, network
services, model weights, or external Python packages.

The harness reads `bench/markhand_web/workload-profile.yaml` and generates a
synthetic tenant/load distribution for the approved envelope:

- 20 orgs
- 10 collections/org
- 5000 documents/collection
- 10 pages/document
- 1M vectors/org
- 20M aggregate vectors
- Zipfian 80/20 tenant load
- peak mixed load: query + ingest + delete

It compares:

- Qdrant shared collection vs cohort collections
- PG no-partition vs bounded hash partitioning (16 buckets)

Every simulated operation carries `org_id` filter metadata. Snapshot/restore
markers are generated with deterministic synthetic timings from seed
`20260718`.

## What it does not claim

This is an offline smoke/decision harness for topology shape only. It does
**not** claim Profile B evidence and does **not** claim
`G0-SLO-QUERY-P99` / 20M mixed-load production evidence.

Production aggregate scale remains blocked until the same topology decision is
revalidated on the approved Profile B target hardware/services with real
Postgres and Qdrant measurements.

## How to run

Self-test:

```bash
python3 bench/markhand_web/scripts/run_scale_topology.py --self-test
```

Full offline run:

```bash
python3 bench/markhand_web/scripts/run_scale_topology.py
```

Outputs:

- `bench/markhand_web/scale/summary.json`
- `bench/markhand_web/reports/scale-topology.md`

`summary.json` includes closure fields. `p0_07_closed` is true only when:

1. ADR 0008 and ADR 0009 exist,
2. both ADRs have `Status: Accepted`,
3. the harness completed,
4. the Phase 1B POC recommendation is recorded, and
5. the git worktree was clean at harness start.

If a development run is dirty, commit the harness/docs/evidence and rerun from
a clean tree to produce closure evidence.
