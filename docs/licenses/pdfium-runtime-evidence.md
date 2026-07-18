# PDFium runtime license evidence

- Inventory id: `pdfium-linux-x64`
- Kind: native-library
- Source: `bblanchon/pdfium-binaries` Linux x64 via `bench/download_pdfium.sh`
- Pinned version: `152.0.7947.0` (`pdfium/VERSION` MAJOR.MINOR.BUILD.PATCH)
- Local artifact checked: `pdfium/lib/libpdfium.so`
- Artifact SHA-256:
  `61c9f745c6296a1050599a99a1ed985036411b591a11bd2a41bafe530ecb4f33`
- Package license file: `pdfium/LICENSE` (MIT for Benoit Blanchon packaging)
- Third-party notices: `pdfium/licenses/` (Abseil, FreeType, ICU, libpng, zlib, etc.)
- License disposition: **MIT** (packaging LICENSE), approved for bundling with
  third-party notices retained
- Redistribution: `source-offer-required` (preserve MIT notice + `licenses/`)
- Bundled: true for Markhand native runtime packaging

## Notes

- Do **not** label this artifact Apache-2.0; the packaging LICENSE is MIT and the
  binary embeds multiple third-party licenses under `pdfium/licenses/`.
- The binary path is gitignored; release jobs must refresh the SHA-256 and VERSION
  pin when `download_pdfium.sh` changes the artifact.
- Profile B / production image cutover should re-run
  `python3 scripts/check-runtime-license-inventory.py` against the packaged tree.
