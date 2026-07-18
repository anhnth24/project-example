# Whisper tiny runtime license evidence

- Inventory id: `ggml-whisper-tiny`
- Kind: model
- Source: `ggerganov/whisper.cpp` GGML tiny model downloaded by
  `bench/download_models.sh`
- Local artifact checked: `models/ggml-tiny.bin`
- Artifact SHA-256:
  `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21`
- Upstream metadata evidence:
  `https://huggingface.co/ggerganov/whisper.cpp/raw/main/README.md`
- Upstream metadata SHA-256:
  `21fd967098804f33fc84e803fb0e5ab7666d71801f4027cf28a65e7af09c1758`
- License disposition: MIT, approved
- Redistribution: allowed
- Bundled: false in this inventory entry; the model is optional and gitignored.

Notes:

- The local model artifact exists in this cloud run but is not source-controlled.
- If a release image bundles the model, change `bundled` to true only after
  pinning the shipped artifact hash in release evidence.
