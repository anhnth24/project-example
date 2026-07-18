# P0-03 desktop baseline

- Git commit: `06c13f250cc2212f1748c9bb8ae7ecae6a9edb83`
- CPU threads visible: 8
- RAM: 47.07 GB
- GPU: none × 0
- Environment role: reduced smoke, not approved Profile B target.
- Conversion and retrieval were independently rerun for deterministic fingerprints.

## Conversion

| Format | Files | Success | Mean CER | Mean WER | Mean ms |
|---|---:|---:|---:|---:|---:|
| audio | 2 | 0 | n/a | n/a | 2.04 |
| csv | 3 | 3 | 0.3128 | 0.3405 | 1.94 |
| docx | 7 | 7 | 0.0000 | 0.0000 | 6.83 |
| html | 3 | 3 | 0.1399 | 0.1703 | 1.99 |
| image_ocr | 3 | 3 | 0.0095 | 0.0211 | 362.46 |
| pdf_native | 3 | 3 | 0.1103 | 0.1072 | 11.16 |
| pdf_scan | 2 | 2 | 0.1149 | 0.1159 | 743.94 |
| pptx | 2 | 2 | 0.1487 | 0.1474 | 2.37 |
| text_legacy | 3 | 3 | 0.0000 | 0.0000 | 1.95 |
| xlsx | 3 | 3 | 0.3062 | 0.3216 | 2.19 |

Audio rows failed dependency admission because no Whisper model was configured;
their durations are failure latency, not transcription performance.

## Local desktop retrieval

- Ranked queries: 238
- No-answer queries: 30
- Recall@5: 0.6408
- Recall@10: 0.7640
- Hit@5: 0.6723
- MRR: 0.5361
- nDCG@10: 0.5805
- Temporal Recall@5: 0.9062
- Current-version Top-1 accuracy: 0.4000
- No-answer accuracy: 0.0000
- Citation document precision: 0.0842
- Citation document recall: 0.6710
- Citation page accuracy (paged sources only): 0.0000
- Citation exact-span accuracy: 0.0000
- Citation token validity: 1.0000
- Answer contains expected text: 0.5672
- Version-citation precision/recall: 0.0 baseline (payload not implemented).

## Interpretation

This report freezes current behavior. It does not claim P0 retrieval, temporal,
capacity, or target-hardware gates pass. Version-aware gold intentionally exposes
the desktop baseline gap before P0-06/P1B implementation.
