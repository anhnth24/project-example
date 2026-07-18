# P0-08 ingest capacity harness

This directory contains local-cpu smoke evidence for P0-08 converter sizing and
ingest backpressure. It is intentionally honest about scope: results from this
runner do not satisfy the Profile B `G0-CAP-INGEST-THROUGHPUT` gate.

## What it measures

`bench/markhand_web/scripts/run_ingest_capacity.py` reads:

- `bench/markhand_web/golden/manifest.json`
- `bench/markhand_web/workload-profile.yaml`
- the converter binary at `FILECONV_BIN`, or `target/release/fileconv` by default

The harness selects every golden document whose format is in
`workloads.ingest.formats`. `conversionOnly` fixtures are skipped only when a
non-`conversionOnly` fixture covers the same format; audio is included because
it is the only audio fixture in the golden workload.

For each selected document it records:

- single-worker and two-worker wall-clock conversion time
- per-format p50/p95/max timing
- pages/sheets/slides when an estimate is available
- peak process-tree RSS from `/proc` when available
- converter failures and timeouts
- deterministic queue-age simulations for normal 1x, recovery 2x normal, and
  peak gate load

## What it does not claim

This harness **does not** claim Profile B capacity, production worker headroom,
or a pass for `G0-CAP-INGEST-THROUGHPUT`. The gate remains blocked until the
same command is run on `on-prem-reference` hardware.

`p0_08_closed` means the P0-08 interim deliverable is present: the harness ran,
the report was written from a clean tree, and the evidence keeps
`targetMatch=false` plus `productionCapacityBlocked=true`. It is not a Profile B
gate-pass flag.

## How to run

Self-test:

```bash
python3 bench/markhand_web/scripts/run_ingest_capacity.py --self-test
```

Full local-cpu run:

```bash
bash bench/markhand_web/scripts/run_ingest_capacity.sh
```

Use a non-default converter binary:

```bash
FILECONV_BIN=/path/to/fileconv bash bench/markhand_web/scripts/run_ingest_capacity.sh
```

If the converter binary is missing, the harness exits clearly and asks for
`FILECONV_BIN` or `cargo build --release`.

Outputs:

- `bench/markhand_web/ingest/summary.json`
- `bench/markhand_web/reports/ingest-capacity.md`
