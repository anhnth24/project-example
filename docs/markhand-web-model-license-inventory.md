# Markhand Web runtime model/native license inventory

Status: P0-09 local close evidence.

## Summary

| id | kind | license | bundled | disposition | redistribution | evidence |
|---|---|---|---|---|---|---|
| `pdfium-linux-x64` | native-library | MIT (+ third-party notices) | true | approved | source-offer-required | `docs/licenses/pdfium-runtime-evidence.md` |
| `tesseract-ocr-system` | native-library | Apache-2.0 | false | approved | allowed | `docs/licenses/tesseract-runtime-evidence.md` |
| `ggml-whisper-tiny` | model | MIT | false | approved | allowed | `docs/licenses/whisper-tiny-runtime-evidence.md` |
| `aiteamvn-vietnamese-embedding` | model | Apache-2.0 | false | approved | allowed | `docs/licenses/aiteamvn-vietnamese-embedding-evidence.md` |
| `phowhisper-ggml` | model | LicenseRef-Unresolved-Exclude | false | excluded | forbidden | `docs/licenses/phowhisper-excluded-evidence.md` |

## Decisions

- PDFium is the only bundled runtime entry in this P0-09 inventory and is
  approved for redistribution. This keeps the bundled approval ratio at 1.0.
- Tesseract is approved but represented here as a system/native prerequisite,
  not as a bundled release artifact. If a release image bundles Tesseract, the
  release must add the shipped binary hash.
- Whisper tiny is approved and locally present in this cloud run, but remains
  optional/non-bundled for P0-09.
- AITeamVN Vietnamese embedding is approved for the quality track and not
  bundled. The inventory checksum records pinned upstream model-card evidence,
  not model weights.
- PhoWhisper is excluded and not bundled. Its license remains unresolved for
  redistribution; the disposition is `excluded`, `bundled=false`, and
  `redistribution=forbidden`.

## Validation

Run:

```bash
python3 scripts/check-runtime-license-inventory.py
```

Expected output:

```json
{"metric": "approved_runtime_licenses", "value": 1.0}
```

P0-09 does not permit bundling a runtime model or native library unless the JSON
inventory has an approved disposition, an allowed license, a valid artifact or
evidence checksum, and an evidence file in this repository.
