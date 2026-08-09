# Ruled-table OCR spike

## Public provenance
- Source id: `official-89-2026-tt-btc`
- Source SHA-256: `952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc`
- Manifest SHA-256: `89511ce0a181be774582075730b502c97170f3166b35acae9a0ca3eb475df6a4`
- Configuration SHA-256: `521efe33c8e128581708c6269e92486799201f59c648f6117048735530b0a495`
- Split: `tuning`
- Access: tuning=6, holdout=0, negative=0

## Bounds
- Cell match IoU: `>= 0.80`
- Cell F1: `>= 0.95`
- Cell CER: `<= 0.05`
- Empty-cell accuracy: `>= 0.98`
- Peak RSS: `< 805306368` bytes
- Page latency: `<= 20` seconds
- Negative false positives: `0`

## Candidate aggregates
| Candidate | Records | Exact grids | TP | FP | FN | Char edits / refs | Word edits / refs | F1 | CER | WER | Empty accuracy | Median s | Max s | Peak RSS | Failures | Negative FP |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `balanced-psm6` | 6 | 2 | 150 | 0 | 134 | 56 / 1703 | 43 / 383 | 0.691244 | 0.032883 | 0.112272 | 0.528169 | 1.199886 | 8.634292 | 69070848 | 4 | 0 |
| `balanced-psm7` | 6 | 2 | 150 | 0 | 134 | 460 / 1703 | 123 / 383 | 0.691244 | 0.270112 | 0.321149 | 0.528169 | 1.209804 | 8.497790 | 69246976 | 4 | 0 |
| `strict-psm6` | 6 | 1 | 40 | 0 | 244 | 25 / 521 | 20 / 119 | 0.246914 | 0.047985 | 0.168067 | 0.140845 | 1.201885 | 3.983995 | 69496832 | 5 | 0 |

## Frozen tuning result
- Winner: none
- Frozen configuration SHA-256: `521efe33c8e128581708c6269e92486799201f59c648f6117048735530b0a495`

## Holdout gate
| Condition | Measured | Threshold | Result |
|---|---:|---:|---|
| Exact table grids | not measured | 3 / 3 | FAIL |
| Cell F1 | not measured | >= 0.95 | FAIL |
| Cell CER | not measured | <= 0.05 | FAIL |
| Empty-cell accuracy | not measured | >= 0.98 | FAIL |
| Negative false positives | not measured | 0 | FAIL |
| Peak RSS bytes | not measured | < 805306368 | FAIL |
| Maximum page latency seconds | not measured | <= 20 | FAIL |

## Decision: STOP

## Limitations
- Supports one table per page with visible rules and no merged cells.
- The corpus is intentionally tiny and is not representative of all documents.
- This spike grants no production or full-document authorization.
