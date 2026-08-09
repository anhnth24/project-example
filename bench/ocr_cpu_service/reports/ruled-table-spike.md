# Ruled-table OCR spike

## Status: BLOCKED / official tuning not run

The official Task 5 tuning command is blocked because the frozen corpus has
`0/12` human-verified annotations. Review metadata remains unchanged.

## Public provenance

- Source id: `official-89-2026-tt-btc`
- Source SHA-256: `952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc`
- Manifest SHA-256: `89511ce0a181be774582075730b502c97170f3166b35acae9a0ca3eb475df6a4`
- Configuration SHA-256: `53882ed34ec756fd2fc9e7bb3ad66ac021c86b26c70a04299f8dd1b1eec0a3f8`
- Vietnamese tessdata SHA-256: `b6b49293d95d0b6dbd8780174627e82c75be957b6f4ed9862155540d6b00bb45`
- Official access: tuning=0, holdout=0, negative=0

## Readiness

- Required before official tuning: `12/12` human-verified annotations
- Current readiness: `0/12`
- Tuning winner: not derived
- Frozen-winner artifact: not created
- Holdout: not authorized and not run
- Gate decision: not measured

An earlier ignored draft-annotation engineering smoke was not official evidence,
did not authorize a measured `STOP` or winner, and has been discarded. This
tracked report contains no smoke aggregates.

## Evidence controls for the eventual official run

- A frozen winner must bind the exact validated bytes of ignored
  `raw/tuning.json`, its recomputed winner ID, and the canonical candidate hash.
- Holdout creates `raw/holdout.started.json` atomically before opening its first
  page; an existing marker blocks every retry, including after a crash.
- Failure evidence records the primary timeout, output, resource, or candidate
  error independently from a concurrent cleanup failure, and aggregates count
  both dimensions.
- The exported report renderer validates every canonical artifact before
  producing any Markdown.

## Bounds reserved for the official run

- Cell match IoU: `>= 0.80`
- Cell F1: `>= 0.95`
- Cell CER: `<= 0.05`
- Empty-cell accuracy: `>= 0.98`
- Peak RSS: `< 805306368` bytes
- Page latency: `<= 20` seconds
- Negative false positives: `0`

## Limitations

- Supports one table per page with visible rules and no merged cells.
- The corpus is intentionally tiny and is not representative of all documents.
- This spike grants no production or full-document authorization.
