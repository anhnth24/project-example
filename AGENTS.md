# Agent guidance

## Cursor Cloud specific instructions

- See `CONTRIBUTING.md` and `docs/runbooks/contributor-setup.md` for the standard toolchain and quality-gate commands. The working products are the shared Rust converter, `fileconv` CLI, `fileconv-mcp`, and Markhand Tauri desktop app; `fileconv-server` and `web/` are currently foundation scaffolds, so the Compose stack is not required for desktop/CLI development.
- The Cloud VM uses GNU `cc`/`c++` and Cargo's GNU linker. Keep this configuration: `whisper-rs-sys` may compile with the image's default Clang but then fail to find `libstdc++` while linking.
- For a headless desktop runtime check, run `xvfb-run -a pnpm --dir app tauri dev`; Tauri starts the Vite server on port 1420 automatically. A harmless DRI3/libEGL warning is expected under Xvfb.
- The snapshot includes Tesseract `vie+eng`, PDFium under `pdfium/`, and higher-quality OCR data under `tessdata_best/`. Audio transcription still requires a model from `bench/download_models.sh`; LLM-backed features require the optional `FILECONV_LLM_*` configuration described in `crates/mcp/README.md`.
- pnpm 10 reports that the `esbuild` install script is ignored. The repository's Vite builds and tests work with its platform package, so do not run the interactive `pnpm approve-builds` during automated setup.
- Before every Rust PR push, run `cargo fmt --all -- --check`, `cargo metadata --locked --format-version 1 --no-deps`, and `python3 scripts/check-dependency-policy.py`. A dependency manifest change must include its `Cargo.lock` update. For a configuration test that expects a specific error, provide valid values for every earlier validation invariant. See `docs/runbooks/contributor-setup.md` for the full CI failure-prevention rules.
