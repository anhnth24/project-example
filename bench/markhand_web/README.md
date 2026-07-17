# Markhand Web benchmark assets

Phase 0 turns architecture assumptions into reviewable evidence. P0-01 defines:

- `workload-profile.yaml`: scale/load envelope and open product decisions;
- `environments/*.yaml`: hardware/environment profiles;
- `gates.yaml`: metric/threshold/command/environment/approver/failure registry;
- `schema/*.schema.json`: machine-readable contracts;
- `reports/environment-report.schema.json`: required benchmark evidence envelope.

The `.yaml` files use JSON-compatible YAML 1.2 so validation needs only Python stdlib.
`proposed` entries may contain explicit `null` values. `approved` workloads/gates may
not: approval means product/hardware owners supplied numeric values and approver.

```bash
make check-markhand-gates
```

## Open decisions

The authoritative decision list lives in `workload-profile.yaml`; this README indexes
it rather than duplicating values. Current owners must resolve organization/document/
vector scale, query/ingest concurrency, target hardware and recovery duration before
P0-01 can be marked Done. No benchmark may claim SLA against a proposed profile.

Fixtures/corpus must be synthetic or de-identified, versioned and license-reviewed.
Large raw benchmark output remains a CI artifact; committed reports contain environment
fingerprint and checksums only.
