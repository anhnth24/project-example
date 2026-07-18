# PhoWhisper excluded runtime evidence

- Inventory id: `phowhisper-ggml`
- Kind: model
- Source: `dongxiat/ggml-PhoWhisper-small` GGML community artifact for
  PhoWhisper-small
- Download script reference: `bench/download_models.sh`
- Local artifact checked: `models/ggml-PhoWhisper-small.bin`
- Local artifact status: absent in this cloud run
- Upstream metadata evidence:
  `https://huggingface.co/dongxiat/ggml-PhoWhisper-small/raw/main/README.md`
- Upstream metadata SHA-256:
  `9a820da888b93d22ff8a850440667f9c4362894aaf99e857d8447651c7bc376b`
- License disposition: `LicenseRef-Unresolved-Exclude`, excluded
- Redistribution: forbidden until a redistributable license is approved
- Bundled: false

Notes:

- P0-09 explicitly excludes PhoWhisper from runtime bundles because the license is
  unresolved for redistribution.
- The inventory checksum for this excluded, non-bundled entry is upstream
  metadata evidence only. It is not a shipped model artifact hash.
- Any future inclusion requires legal approval, a concrete redistributable
  license, and a real shipped artifact hash.
