# Bitonal PDF OCR calibration

This report contains additive counts and hashes only. It does not contain recognized or private-reference text.

## Provenance

- Source SHA-256: `952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc`
- Private reference: runtime-validated against ignored raw provenance; hash/path omitted from tracked output
- Config SHA-256: `5d83715fc65e4cbfff51c6d53a8c615a2657fe8dd4c4e519f52d9797162f9577`
- Binary SHA-256: `8d7d179f5cc5ed6ef78c90efb3a99de8a5d50123a335974e749dd994633f5ff7`
- Classifier SHA-256: `c7b7a03bcc09ad88279d41d10bf3980dea167c1876d5748a985f0630dc3200d2`
- PDFium SHA-256: `0c6b5e32e878b04784ced0995dc42c6f106ad02348b2fd3aa89d7886e075b66e`
- Host descriptor SHA-256: `02cc5ec2f3f010d6832aa8f1d874dc5e59a7442210cc14844b9acf37091240a1`
- Toolchain descriptor SHA-256: `b34c9920bf915eb870eb856cbc7e18aac0000466fb0393116f15f9d69a10e6b9`
- Build command: `CC=gcc CXX=g++ cargo build --release -p fileconv-cli --no-default-features`
- Approved pages opened: 22
- Holdout pages opened: 0
- OCR executions: 88
- Rust classifier diagnostics: 22

## Fixed bounds

- CPU threads: 1
- Timeout per page: 180 seconds
- Maximum output bytes per stream: 1048576
- Maximum sampled process-tree peak RSS bytes (strictly below): 805306368
- Process-tree sample interval: 10 ms

A successful page marker below means one unique successful candidate-page record for an approved page; OCR text is not retained.

## Gate summary

- `baseline_id`: `legacy-best-vie-eng`
- `winner_id`: `null`
- `tied_ids`: none
- `winner_configuration_sha256`: `null`

| Candidate | Matched legacy | Eligible | Activations | Character disagreement | Word disagreement | Median seconds | Sampled process-tree peak RSS bytes | Successful page markers | Failures | Resource violations | Configuration SHA-256 | Disqualifications |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| `legacy-best-vie-eng` | `legacy-best-vie-eng` | no | 0 | 0.852978% | 2.943131% | 2.123984 | 160534528 | 22 | 0 | 0 | `f21232ffba038eb1fe78a34b7b8e8d03381bc042ea03d8529aa9f8e516a59d81` | legacy_control_not_preserve_candidate, aggregate_character_disagreement_not_lower_than_matched_legacy |
| `preserve-best-vie-eng` | `legacy-best-vie-eng` | no | 22 | 1.194594% | 4.386215% | 2.154243 | 188563456 | 22 | 0 | 0 | `a0ebf1ddbaed1edcee64705f5571809526b33984388df34cc9d490d8b2b9ce92` | aggregate_character_disagreement_not_lower_than_matched_legacy |
| `legacy-best-vie` | `legacy-best-vie` | no | 0 | 0.693840% | 2.373493% | 1.757421 | 156352512 | 22 | 0 | 0 | `ca1a7d4a08b3272d6c582d593988c2628473d5271ddad2489791b4aed2cb4e05` | legacy_control_not_preserve_candidate, aggregate_character_disagreement_not_lower_than_matched_legacy |
| `preserve-best-vie` | `legacy-best-vie` | no | 22 | 0.997263% | 3.740625% | 1.707381 | 155762688 | 22 | 0 | 0 | `22641412a9e42855cca7716495bd94f384236ffb9cffaf8608aa1e277cbe0e7a` | aggregate_character_disagreement_not_lower_than_matched_legacy |

## Pages 1–20 additive disagreement counts

| Page | Candidate | Character edits | Reference characters | Character disagreement | Delta vs matched legacy (percentage points) | Word edits | Reference words |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | `legacy-best-vie-eng` | 117 | 1774 | 6.595265% | 0.000000 | 40 | 379 |
| 1 | `preserve-best-vie-eng` | 128 | 1774 | 7.215333% | 0.620068 | 53 | 379 |
| 1 | `legacy-best-vie` | 108 | 1774 | 6.087937% | 0.000000 | 36 | 379 |
| 1 | `preserve-best-vie` | 114 | 1774 | 6.426156% | 0.338219 | 44 | 379 |
| 2 | `legacy-best-vie-eng` | 15 | 1977 | 0.758725% | 0.000000 | 14 | 436 |
| 2 | `preserve-best-vie-eng` | 21 | 1977 | 1.062215% | 0.303490 | 21 | 436 |
| 2 | `legacy-best-vie` | 8 | 1977 | 0.404654% | 0.000000 | 8 | 436 |
| 2 | `preserve-best-vie` | 17 | 1977 | 0.859889% | 0.455235 | 17 | 436 |
| 3 | `legacy-best-vie-eng` | 9 | 2384 | 0.377517% | 0.000000 | 8 | 530 |
| 3 | `preserve-best-vie-eng` | 11 | 2384 | 0.461409% | 0.083893 | 11 | 530 |
| 3 | `legacy-best-vie` | 5 | 2384 | 0.209732% | 0.000000 | 6 | 530 |
| 3 | `preserve-best-vie` | 10 | 2384 | 0.419463% | 0.209732 | 10 | 530 |
| 4 | `legacy-best-vie-eng` | 16 | 2169 | 0.737667% | 0.000000 | 16 | 477 |
| 4 | `preserve-best-vie-eng` | 31 | 2169 | 1.429230% | 0.691563 | 29 | 477 |
| 4 | `legacy-best-vie` | 13 | 2169 | 0.599355% | 0.000000 | 13 | 477 |
| 4 | `preserve-best-vie` | 25 | 2169 | 1.152605% | 0.553250 | 25 | 477 |
| 5 | `legacy-best-vie-eng` | 13 | 2335 | 0.556745% | 0.000000 | 12 | 514 |
| 5 | `preserve-best-vie-eng` | 22 | 2335 | 0.942184% | 0.385439 | 22 | 514 |
| 5 | `legacy-best-vie` | 10 | 2335 | 0.428266% | 0.000000 | 10 | 514 |
| 5 | `preserve-best-vie` | 24 | 2335 | 1.027837% | 0.599572 | 23 | 514 |
| 6 | `legacy-best-vie-eng` | 18 | 2317 | 0.776867% | 0.000000 | 18 | 525 |
| 6 | `preserve-best-vie-eng` | 37 | 2317 | 1.596893% | 0.820026 | 36 | 525 |
| 6 | `legacy-best-vie` | 16 | 2317 | 0.690548% | 0.000000 | 16 | 525 |
| 6 | `preserve-best-vie` | 32 | 2317 | 1.381096% | 0.690548 | 31 | 525 |
| 7 | `legacy-best-vie-eng` | 45 | 2578 | 1.745539% | 0.000000 | 41 | 587 |
| 7 | `preserve-best-vie-eng` | 46 | 2578 | 1.784329% | 0.038790 | 43 | 587 |
| 7 | `legacy-best-vie` | 37 | 2578 | 1.435221% | 0.000000 | 37 | 587 |
| 7 | `preserve-best-vie` | 41 | 2578 | 1.590380% | 0.155159 | 41 | 587 |
| 8 | `legacy-best-vie-eng` | 25 | 2481 | 1.007658% | 0.000000 | 23 | 571 |
| 8 | `preserve-best-vie-eng` | 40 | 2481 | 1.612253% | 0.604595 | 38 | 571 |
| 8 | `legacy-best-vie` | 18 | 2481 | 0.725514% | 0.000000 | 16 | 571 |
| 8 | `preserve-best-vie` | 31 | 2481 | 1.249496% | 0.523982 | 31 | 571 |
| 9 | `legacy-best-vie-eng` | 19 | 2337 | 0.813008% | 0.000000 | 19 | 529 |
| 9 | `preserve-best-vie-eng` | 40 | 2337 | 1.711596% | 0.898588 | 39 | 529 |
| 9 | `legacy-best-vie` | 16 | 2337 | 0.684638% | 0.000000 | 16 | 529 |
| 9 | `preserve-best-vie` | 40 | 2337 | 1.711596% | 1.026958 | 39 | 529 |
| 10 | `legacy-best-vie-eng` | 6 | 2695 | 0.222635% | 0.000000 | 6 | 608 |
| 10 | `preserve-best-vie-eng` | 7 | 2695 | 0.259740% | 0.037106 | 6 | 608 |
| 10 | `legacy-best-vie` | 7 | 2695 | 0.259740% | 0.000000 | 7 | 608 |
| 10 | `preserve-best-vie` | 6 | 2695 | 0.222635% | -0.037106 | 5 | 608 |
| 11 | `legacy-best-vie-eng` | 22 | 2109 | 1.043148% | 0.000000 | 20 | 470 |
| 11 | `preserve-best-vie-eng` | 30 | 2109 | 1.422475% | 0.379327 | 27 | 470 |
| 11 | `legacy-best-vie` | 21 | 2109 | 0.995733% | 0.000000 | 17 | 470 |
| 11 | `preserve-best-vie` | 29 | 2109 | 1.375059% | 0.379327 | 26 | 470 |
| 12 | `legacy-best-vie-eng` | 4 | 2368 | 0.168919% | 0.000000 | 4 | 532 |
| 12 | `preserve-best-vie-eng` | 8 | 2368 | 0.337838% | 0.168919 | 7 | 532 |
| 12 | `legacy-best-vie` | 1 | 2368 | 0.042230% | 0.000000 | 1 | 532 |
| 12 | `preserve-best-vie` | 4 | 2368 | 0.168919% | 0.126689 | 4 | 532 |
| 13 | `legacy-best-vie-eng` | 7 | 2738 | 0.255661% | 0.000000 | 7 | 604 |
| 13 | `preserve-best-vie-eng` | 5 | 2738 | 0.182615% | -0.073046 | 5 | 604 |
| 13 | `legacy-best-vie` | 5 | 2738 | 0.182615% | 0.000000 | 5 | 604 |
| 13 | `preserve-best-vie` | 3 | 2738 | 0.109569% | -0.073046 | 3 | 604 |
| 14 | `legacy-best-vie-eng` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `preserve-best-vie-eng` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `legacy-best-vie` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `preserve-best-vie` | 4 | 2468 | 0.162075% | -0.081037 | 4 | 549 |
| 15 | `legacy-best-vie-eng` | 21 | 2296 | 0.914634% | 0.000000 | 19 | 532 |
| 15 | `preserve-best-vie-eng` | 16 | 2296 | 0.696864% | -0.217770 | 13 | 532 |
| 15 | `legacy-best-vie` | 15 | 2296 | 0.653310% | 0.000000 | 15 | 532 |
| 15 | `preserve-best-vie` | 10 | 2296 | 0.435540% | -0.217770 | 10 | 532 |
| 16 | `legacy-best-vie-eng` | 8 | 2258 | 0.354296% | 0.000000 | 7 | 505 |
| 16 | `preserve-best-vie-eng` | 10 | 2258 | 0.442870% | 0.088574 | 9 | 505 |
| 16 | `legacy-best-vie` | 3 | 2258 | 0.132861% | 0.000000 | 3 | 505 |
| 16 | `preserve-best-vie` | 8 | 2258 | 0.354296% | 0.221435 | 8 | 505 |
| 17 | `legacy-best-vie-eng` | 19 | 2261 | 0.840336% | 0.000000 | 19 | 509 |
| 17 | `preserve-best-vie-eng` | 41 | 2261 | 1.813357% | 0.973021 | 41 | 509 |
| 17 | `legacy-best-vie` | 11 | 2261 | 0.486510% | 0.000000 | 12 | 509 |
| 17 | `preserve-best-vie` | 31 | 2261 | 1.371075% | 0.884564 | 32 | 509 |
| 18 | `legacy-best-vie-eng` | 13 | 2429 | 0.535200% | 0.000000 | 14 | 540 |
| 18 | `preserve-best-vie-eng` | 27 | 2429 | 1.111569% | 0.576369 | 24 | 540 |
| 18 | `legacy-best-vie` | 10 | 2429 | 0.411692% | 0.000000 | 10 | 540 |
| 18 | `preserve-best-vie` | 14 | 2429 | 0.576369% | 0.164677 | 15 | 540 |
| 19 | `legacy-best-vie-eng` | 12 | 2600 | 0.461538% | 0.000000 | 10 | 574 |
| 19 | `preserve-best-vie-eng` | 26 | 2600 | 1.000000% | 0.538462 | 22 | 574 |
| 19 | `legacy-best-vie` | 10 | 2600 | 0.384615% | 0.000000 | 9 | 574 |
| 19 | `preserve-best-vie` | 17 | 2600 | 0.653846% | 0.269231 | 17 | 574 |
| 20 | `legacy-best-vie-eng` | 7 | 2555 | 0.273973% | 0.000000 | 7 | 562 |
| 20 | `preserve-best-vie-eng` | 11 | 2555 | 0.430528% | 0.156556 | 10 | 562 |
| 20 | `legacy-best-vie` | 7 | 2555 | 0.273973% | 0.000000 | 7 | 562 |
| 20 | `preserve-best-vie` | 10 | 2555 | 0.391389% | 0.117417 | 9 | 562 |

## Page 60 accented/unaccented word-pair diagnostics

| Candidate | Pair | Accented count | Unaccented count | Unaccented ratio |
|---|---|---:|---:|---:|
| `legacy-best-vie-eng` | `dieu` | 1 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `duoc` | 2 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `luat` | 1 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `nghiep` | 2 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `nguoi` | 7 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `nuoc` | 2 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `quyet` | 1 | 0 | 0.000000% |
| `legacy-best-vie-eng` | `truong` | 3 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `dieu` | 0 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `duoc` | 2 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `luat` | 1 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `nghiep` | 2 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `nguoi` | 7 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `nuoc` | 2 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `quyet` | 1 | 0 | 0.000000% |
| `preserve-best-vie-eng` | `truong` | 3 | 0 | 0.000000% |
| `legacy-best-vie` | `dieu` | 1 | 0 | 0.000000% |
| `legacy-best-vie` | `duoc` | 2 | 0 | 0.000000% |
| `legacy-best-vie` | `luat` | 1 | 0 | 0.000000% |
| `legacy-best-vie` | `nghiep` | 2 | 0 | 0.000000% |
| `legacy-best-vie` | `nguoi` | 7 | 0 | 0.000000% |
| `legacy-best-vie` | `nuoc` | 2 | 0 | 0.000000% |
| `legacy-best-vie` | `quyet` | 1 | 0 | 0.000000% |
| `legacy-best-vie` | `truong` | 3 | 0 | 0.000000% |
| `preserve-best-vie` | `dieu` | 0 | 0 | 0.000000% |
| `preserve-best-vie` | `duoc` | 2 | 0 | 0.000000% |
| `preserve-best-vie` | `luat` | 1 | 0 | 0.000000% |
| `preserve-best-vie` | `nghiep` | 2 | 0 | 0.000000% |
| `preserve-best-vie` | `nguoi` | 7 | 0 | 0.000000% |
| `preserve-best-vie` | `nuoc` | 2 | 0 | 0.000000% |
| `preserve-best-vie` | `quyet` | 1 | 0 | 0.000000% |
| `preserve-best-vie` | `truong` | 3 | 0 | 0.000000% |

## Page 450 coverage diagnostics

| Candidate | Digit characters | Digit-stream SHA-256 | Legal identifiers | Non-whitespace characters | Suspicious characters |
|---|---:|---|---:|---:|---:|
| `legacy-best-vie-eng` | 95 | `4604bf4e8b7b8bdc7b8ec0a9816a6729104445a4e53ad96e4b9c73d3e99eea95` | 0 | 1862 | 62 |
| `preserve-best-vie-eng` | 96 | `0cd3916f269b7602853d24fce28900e9c7e1f2047f6540e6099879a3830a44e8` | 0 | 1929 | 67 |
| `legacy-best-vie` | 101 | `8962f08baff14ddb6b65397b0de9c1e005b972f19615e3bbad0e30de5c21ed49` | 0 | 1862 | 55 |
| `preserve-best-vie` | 103 | `2a929ad5430194c15f97253b8244700bade7aa4754c3cbe3ce74fa5aa37244b9` | 1 | 1928 | 59 |

## Candidate semantics

| Candidate | Mode | Tessdata | Languages |
|---|---|---|---|
| `legacy-best-vie-eng` | `legacy` | `best` | `vie+eng` |
| `preserve-best-vie-eng` | `preserve-near-bitonal` | `best` | `vie+eng` |
| `legacy-best-vie` | `legacy` | `best` | `vie` |
| `preserve-best-vie` | `preserve-near-bitonal` | `best` | `vie` |

## Exact Rust classifier activation evidence

| Page | Extreme ratio | Ink ratio | Qualifies | Preserve activated | Render SHA-256 |
|---:|---:|---:|---:|---:|---|
| 1 | 92.981844341% | 1.416313538% | yes | yes | `443f38ed0547255c625f1d7ba8279057300718b846168044c3abf61925173d0d` |
| 2 | 92.214812969% | 1.034608343% | yes | yes | `4ef12e4af25d7d19a1dbdb7be0f802c70215bb646f9db22e6c602f2dbd06b5c1` |
| 3 | 95.921439070% | 6.245088681% | yes | yes | `4edc153890951d41f131bb8fcca493196c6199e70ed171308de5d86bf8f3c47f` |
| 4 | 96.463209105% | 5.612387270% | yes | yes | `59686dcf2746a75b3dd3d671f53277877fe080e321862313db4654cf95123f8b` |
| 5 | 95.984015338% | 5.957103640% | yes | yes | `c87a087c0c8213c6bb8d1d42aea606a27aad8cd2032fb57c1a9e4e6602343a8b` |
| 6 | 96.021892704% | 6.339140377% | yes | yes | `c72bb937d0859b25ef5f3ff14d48a01fac53bd0f3c8a266f6f0f75f68413f97a` |
| 7 | 95.656986116% | 6.707986526% | yes | yes | `5338a9ff41bf97bd1be79f24cef110765cf10a869276c56ac5f64c35043083b4` |
| 8 | 95.773238827% | 6.436963375% | yes | yes | `4a89018a41a7ef00797b37d4445010b088ee505d316b86d9c396919992deeb73` |
| 9 | 95.977877760% | 6.052605505% | yes | yes | `4b261c1c385ccdb8d7f9ee3f07811672cab703c7ff10554ea813fc40d3a91a24` |
| 10 | 95.035384923% | 6.774438992% | yes | yes | `83cf78d847f8f01adfe57370903a0ff3f08b4a765efb9454a28097707770d8f4` |
| 11 | 96.465995164% | 5.524372364% | yes | yes | `4e1c02e5928fb760edd975b44042b615e93ac1af2787ffeb583bc7a1bb03fddd` |
| 12 | 96.055807224% | 6.256104731% | yes | yes | `c48ab74c935ebeee4c4fb2e8fdd91dc5702e6a94a7179593b9c0bf2aa9fe8e46` |
| 13 | 94.723588119% | 6.651007303% | yes | yes | `1bb400c2503b61021e75230ccca7460e9b05e807d52b1e612738bb2161e44966` |
| 14 | 95.827021757% | 6.468825750% | yes | yes | `e86a64c5fe6c8d32ed3d9765d62b9ab622133c36e84d25cc65ed414ac13fbdda` |
| 15 | 95.752607115% | 5.681561089% | yes | yes | `2a2e9733ca9fc42e07c27ae58e2dc2052c5619dfc1a612ad27aba3da86aa4181` |
| 16 | 96.157527013% | 6.000639858% | yes | yes | `4819f2ba1feb9c9371e892813a2195876b325e962ffba26d0d0974623a2a8447` |
| 17 | 96.141956919% | 5.794903625% | yes | yes | `d7927cf34093e491405bf5fca4fa1445faa4745fb32ad24f49f174768e5a4b08` |
| 18 | 95.878279457% | 6.460375362% | yes | yes | `f3e62106d6d0164ad13aea83486a2af6efa38ab60f46816d323ef5ce214aa6cc` |
| 19 | 95.526621186% | 6.736437633% | yes | yes | `747bf4c9a4f810c96507e6cf1ef46cd40c53bec506fd5ce4e40997a535382889` |
| 20 | 95.218699500% | 6.536064514% | yes | yes | `c70c0dcd6988621cd97feabaf2a6408a917fcb855fa9ccaa0bd3b5349a1630bf` |
| 60 | 96.417316985% | 5.468018428% | yes | yes | `41314b2b8c8316712d4f9b40055b0a2a272cba24e52043d5d32f0c155b9b90ef` |
| 450 | 96.779655670% | 4.293463839% | yes | yes | `b732c90da799d3191198e880762466d33dc3f707492fd162320082caa2ad3fc4` |

## Conclusion

- Decision: **STOP; no preserve candidate passed every corrected gate**.
- Winner: `null`.
- 839-page run executed: no.
- Task 4 execution remains prohibited until this corrected calibration is independently reviewed.

## Limitations and corrections

- Pages 1–20 use a private, non-human-verified acceptance reference; it is not ground truth and the disagreement metric is not CER.
- Pages 60 and 450 have no verified transcription; their diagnostics are coverage/error proxies only.
- Activation evidence above is bound to each exact render, the release binary, and the frozen Rust classifier constants.
- The prior 98.5% run never activated preserve mode and is invalid for the preprocessing hypothesis; it is superseded by this calibration.
- The corrected exact 2×2 tessdata_best matrix separates preprocessing effects from language effects.
- Sampled process-tree peak RSS uses one strict '< 768 MiB' rule for per-record violations and aggregate eligibility.
