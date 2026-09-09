# INTERN-33 — Build artifact analysis (issue #371)

Read-only audit of release build times, binary size, incremental compile behavior, and top symbols. **No changes** to `Cargo.toml`, profiles, or committed binaries.

**Branch:** `intern/33-build-artifact-analysis`  
**Date:** 2026-08-14  
**Machine:** WSL2 Linux (single run; numbers vary by CPU/disk cache)

## Scope

| Build target | Command |
|--------------|---------|
| CLI lean | `cargo build --release -p fileconv-cli --no-default-features` |
| CLI default | `cargo build --release -p fileconv-cli` (features: `audio`, `llm`) |
| Full workspace | `cargo build --release` (6 members incl. Tauri `markhand`) |

Each cold scoped build preceded by `cargo clean`. Incremental tests run on warm cache after CLI default build.

## Build times

| Scenario | Wall time | Notes |
|----------|----------:|-------|
| Cold CLI lean | **372 s** (~6m 12s) | No whisper/LLM deps compiled |
| Cold CLI default | **481 s** (~8m 01s) | Compiles `whisper-rs-sys`, `whisper-rs`, `symphonia`, TLS stack |
| Cold full workspace | **952 s** (~15m 52s) | + Tauri/GTK, server, MCP, knowledge crates |
| Incremental (touch `core/lib.rs`) | **47 s** | Rebuilds `fileconv-core` + `fileconv-cli` |
| Incremental (touch `cli/main.rs`) | **289 s** | Rebuilds `fileconv-cli` only; release link still heavy |

Lean saves **~109 s (29%)** vs default cold. Full workspace is **~2×** CLI default.

## Binary size

| Artifact | Size | Tool |
|----------|-----:|------|
| `fileconv` lean | **21.5 MiB** | `cargo bloat --no-default-features` |
| `fileconv` default | **31.2 MiB** | `cargo bloat` (default features) |
| `markhand` (desktop) | **48 MiB** | `ls -lh target/release/markhand` |

Default CLI is **~9.7 MiB (+45%)** larger than lean. Desktop binary adds Tauri/WebView/GTK + knowledge/RAG stack.

## Top symbols (`cargo bloat -n 20`)

### Default CLI

| % file | Crate | Symbol (abbrev.) |
|-------:|-------|------------------|
| 1.3% | lopdf | `Glyph::from_name` |
| 1.0% | pdf_extract | `name_to_unicode` |
| 0.3% | pdf_inspector | glyph map |
| 0.1% | whisper_rs_sys | `ggml_gemm_*`, `whisper_full_with_state` |
| 0.1% | fileconv | `main` |
| 0.1% | html5ever | tree builder |
| 0.1% | pdfium_render | `DynamicPdfiumBindings::new` |

`.text` section: **15.0 MiB**; 29 267 methods in tail bucket.

### Lean CLI

Same PDF/HTML/DOCX/XLSX symbols dominate; **no `whisper_rs_sys` entries**. `.text`: **10.3 MiB**; 20 973 methods in tail bucket.

PDF pipeline (`lopdf`, `pdf_extract`, `pdf_inspector`, `pdfium_render`) is the largest *shared* footprint; whisper/LLM explains most of the lean→default gap.

## Bottleneck crates (compile)

1. **`whisper-rs-sys`** — C++/cmake whisper.cpp; only with `audio` feature; runbook: `cargo clean -p whisper-rs-sys` if stale.
2. **Tauri / GTK** — full workspace only; long dependency chain before `fileconv-desktop`.
3. **`ring` / HTTP stack** — pulled by LLM/vision paths (`llm` feature).
4. **`pdf-inspector` + `pdfium-render`** — always built for CLI; heavy runtime symbols.

## Cache efficiency

- Core touch incremental ≈ **10%** of cold default time — good reuse of dependency artifacts.
- CLI-only touch still **~289 s** in release (full relink + LTO-unaware release profile).
- Separate `cargo clean` per cold benchmark is required; otherwise whisper native cache masks true cold cost.

## Recommendation (no code change in this issue)

**Build `fileconv-cli --no-default-features` for POC workers, server images, and CI jobs that only need document conversion** — already noted in [`crates/cli/Cargo.toml`](../crates/cli/Cargo.toml).

Measured on this host:

- **29% faster** cold compile
- **31% smaller** binary (21.5 vs 31.2 MiB)
- Avoids bundling whisper.cpp and accidental PhoWhisper in sandbox images

Follow-ups (separate PRs): scoped CI `-p fileconv-cli`; optional release LTO/strip for desktop bundle only.

## Commands reference

```bash
cargo clean && /usr/bin/time cargo build --release -p fileconv-cli --no-default-features
cargo clean && /usr/bin/time cargo build --release -p fileconv-cli
touch crates/core/src/lib.rs && /usr/bin/time cargo build --release -p fileconv-cli
cargo bloat --release -p fileconv-cli --bin fileconv -n 20
cargo bloat --release -p fileconv-cli --bin fileconv --no-default-features -n 20
cargo clean && /usr/bin/time cargo build --release
```

## Related docs

- [`CLAUDE.md`](../CLAUDE.md) — whisper first compile ~1–2 min
- [`docs/runbooks/contributor-setup.md`](../docs/runbooks/contributor-setup.md) — `cargo clean -p whisper-rs-sys`
- [`crates/cli/Cargo.toml`](../crates/cli/Cargo.toml) — default features vs lean worker builds
