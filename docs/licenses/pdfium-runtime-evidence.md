# PDFium runtime license evidence

- Inventory id: `pdfium-linux-x64`
- Kind: native-library
- Source: `bblanchon/pdfium-binaries` Linux x64 runtime used by
  `bench/download_pdfium.sh`
- Local artifact checked: `pdfium/lib/libpdfium.so`
- Artifact SHA-256:
  `61c9f745c6296a1050599a99a1ed985036411b591a11bd2a41bafe530ecb4f33`
- License disposition: Apache-2.0, approved
- Redistribution: allowed
- Bundled: true for Markhand native runtime packaging

Notes:

- The artifact path is gitignored and is not stored in source control.
- P0-09 inventory uses the real local binary hash present in this cloud run.
- Future release jobs must refresh this entry if the PDFium package changes.
