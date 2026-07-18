# Tesseract runtime license evidence

- Inventory id: `tesseract-ocr-system`
- Kind: native-library
- Source: Ubuntu package `tesseract-ocr`
- Version observed: `5.3.4-1build5` (`tesseract 5.3.4`)
- Local artifact checked: `/usr/bin/tesseract`
- Artifact SHA-256:
  `9f831cab7525c3dab04af41bda35182af7ea1df9dceeaaa2f3bf207ac45c06a5`
- Package copyright evidence:
  `/usr/share/doc/tesseract-ocr/copyright`
- License disposition: Apache-2.0, approved
- Redistribution: allowed
- Bundled: false in this inventory entry; treated as a system/native runtime
  prerequisite unless a release package pins its own bundle hash.

Notes:

- The Debian copyright file states `Files: *` and `License: Apache-2.0`.
- If desktop release packaging bundles a different Tesseract executable, add a
  release-specific inventory entry with that artifact hash before shipping.
