# Bitonal PDF OCR calibration

This report contains additive counts and hashes only. It does not contain recognized or private-reference text.

## Provenance

- Source SHA-256: `952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc`
- Private reference SHA-256: `b1f07425d9d82e129bfce7cf52570af14bfbd82da93daf8cb7721e01625cedd8`
- Config SHA-256: `e37f2f60eb94211656e8c41d0a9ef438ee0dd0f9f6902388b9009d2332568758`
- Binary SHA-256: `1584939f59a20d0be2c9879505f8b65f73679a9dd058c9175c54e7febf68fb86`
- PDFium SHA-256: `0c6b5e32e878b04784ced0995dc42c6f106ad02348b2fd3aa89d7886e075b66e`
- Host descriptor SHA-256: `02cc5ec2f3f010d6832aa8f1d874dc5e59a7442210cc14844b9acf37091240a1`
- Toolchain descriptor SHA-256: `b34c9920bf915eb870eb856cbc7e18aac0000466fb0393116f15f9d69a10e6b9`
- Build command: `CC=gcc CXX=g++ cargo build --release -p fileconv-cli --no-default-features`
- Approved pages opened: 22
- Holdout pages opened: 0
- OCR executions: 88

## Fixed bounds

- CPU threads: 1
- Timeout per page: 180 seconds
- Maximum output bytes per stream: 1048576
- Maximum sampled RSS bytes: 805306368
- Process-tree sample interval: 10 ms

A successful page marker below means one unique successful candidate-page record for an approved page; OCR text is not retained.

## Gate summary

- `baseline_id`: `baseline-system-fast`
- `winner_id`: `null`
- `tied_ids`: none
- `winner_configuration_sha256`: `null`

| Candidate | Eligible | Character disagreement | Word disagreement | Median seconds | Peak RSS bytes | Successful page markers | Failures | Resource violations | Configuration SHA-256 | Disqualifications |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| `baseline-system-fast` | no | 1.088502% | 3.797589% | 1.647462 | 156545024 | 22 | 0 | 0 | `82e434c491956a5c3f1747b882717921422547bad06c4013e830e6d90d3d7bc2` | aggregate_character_disagreement_not_lower_than_baseline |
| `baseline-best` | no | 0.852978% | 2.943131% | 2.153830 | 160514048 | 22 | 0 | 0 | `f21232ffba038eb1fe78a34b7b8e8d03381bc042ea03d8529aa9f8e516a59d81` | page_60_accent_proxy_no_strict_improvement, page_450_digit_sequence_count_regression |
| `bitonal-best-vie-eng` | no | 0.852978% | 2.943131% | 2.132419 | 160636928 | 22 | 0 | 0 | `a0ebf1ddbaed1edcee64705f5571809526b33984388df34cc9d490d8b2b9ce92` | page_60_accent_proxy_no_strict_improvement, page_450_digit_sequence_count_regression |
| `bitonal-best-vie` | no | 0.693840% | 2.373493% | 1.751307 | 156332032 | 22 | 0 | 0 | `22641412a9e42855cca7716495bd94f384236ffb9cffaf8608aa1e277cbe0e7a` | page_60_accent_proxy_no_strict_improvement, page_450_digit_sequence_count_regression |

## Pages 1–20 additive disagreement counts

| Page | Candidate | Character edits | Reference characters | Character disagreement | Delta vs baseline (percentage points) | Word edits | Reference words |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | `baseline-system-fast` | 130 | 1774 | 7.328072% | 0.000000 | 53 | 379 |
| 1 | `baseline-best` | 117 | 1774 | 6.595265% | -0.732807 | 40 | 379 |
| 1 | `bitonal-best-vie-eng` | 117 | 1774 | 6.595265% | -0.732807 | 40 | 379 |
| 1 | `bitonal-best-vie` | 108 | 1774 | 6.087937% | -1.240135 | 36 | 379 |
| 2 | `baseline-system-fast` | 18 | 1977 | 0.910470% | 0.000000 | 17 | 436 |
| 2 | `baseline-best` | 15 | 1977 | 0.758725% | -0.151745 | 14 | 436 |
| 2 | `bitonal-best-vie-eng` | 15 | 1977 | 0.758725% | -0.151745 | 14 | 436 |
| 2 | `bitonal-best-vie` | 8 | 1977 | 0.404654% | -0.505817 | 8 | 436 |
| 3 | `baseline-system-fast` | 10 | 2384 | 0.419463% | 0.000000 | 10 | 530 |
| 3 | `baseline-best` | 9 | 2384 | 0.377517% | -0.041946 | 8 | 530 |
| 3 | `bitonal-best-vie-eng` | 9 | 2384 | 0.377517% | -0.041946 | 8 | 530 |
| 3 | `bitonal-best-vie` | 5 | 2384 | 0.209732% | -0.209732 | 6 | 530 |
| 4 | `baseline-system-fast` | 13 | 2169 | 0.599355% | 0.000000 | 12 | 477 |
| 4 | `baseline-best` | 16 | 2169 | 0.737667% | 0.138313 | 16 | 477 |
| 4 | `bitonal-best-vie-eng` | 16 | 2169 | 0.737667% | 0.138313 | 16 | 477 |
| 4 | `bitonal-best-vie` | 13 | 2169 | 0.599355% | 0.000000 | 13 | 477 |
| 5 | `baseline-system-fast` | 15 | 2335 | 0.642398% | 0.000000 | 14 | 514 |
| 5 | `baseline-best` | 13 | 2335 | 0.556745% | -0.085653 | 12 | 514 |
| 5 | `bitonal-best-vie-eng` | 13 | 2335 | 0.556745% | -0.085653 | 12 | 514 |
| 5 | `bitonal-best-vie` | 10 | 2335 | 0.428266% | -0.214133 | 10 | 514 |
| 6 | `baseline-system-fast` | 27 | 2317 | 1.165300% | 0.000000 | 26 | 525 |
| 6 | `baseline-best` | 18 | 2317 | 0.776867% | -0.388433 | 18 | 525 |
| 6 | `bitonal-best-vie-eng` | 18 | 2317 | 0.776867% | -0.388433 | 18 | 525 |
| 6 | `bitonal-best-vie` | 16 | 2317 | 0.690548% | -0.474752 | 16 | 525 |
| 7 | `baseline-system-fast` | 43 | 2578 | 1.667960% | 0.000000 | 39 | 587 |
| 7 | `baseline-best` | 45 | 2578 | 1.745539% | 0.077580 | 41 | 587 |
| 7 | `bitonal-best-vie-eng` | 45 | 2578 | 1.745539% | 0.077580 | 41 | 587 |
| 7 | `bitonal-best-vie` | 37 | 2578 | 1.435221% | -0.232739 | 37 | 587 |
| 8 | `baseline-system-fast` | 28 | 2481 | 1.128577% | 0.000000 | 27 | 571 |
| 8 | `baseline-best` | 25 | 2481 | 1.007658% | -0.120919 | 23 | 571 |
| 8 | `bitonal-best-vie-eng` | 25 | 2481 | 1.007658% | -0.120919 | 23 | 571 |
| 8 | `bitonal-best-vie` | 18 | 2481 | 0.725514% | -0.403063 | 16 | 571 |
| 9 | `baseline-system-fast` | 28 | 2337 | 1.198117% | 0.000000 | 23 | 529 |
| 9 | `baseline-best` | 19 | 2337 | 0.813008% | -0.385109 | 19 | 529 |
| 9 | `bitonal-best-vie-eng` | 19 | 2337 | 0.813008% | -0.385109 | 19 | 529 |
| 9 | `bitonal-best-vie` | 16 | 2337 | 0.684638% | -0.513479 | 16 | 529 |
| 10 | `baseline-system-fast` | 12 | 2695 | 0.445269% | 0.000000 | 10 | 608 |
| 10 | `baseline-best` | 6 | 2695 | 0.222635% | -0.222635 | 6 | 608 |
| 10 | `bitonal-best-vie-eng` | 6 | 2695 | 0.222635% | -0.222635 | 6 | 608 |
| 10 | `bitonal-best-vie` | 7 | 2695 | 0.259740% | -0.185529 | 7 | 608 |
| 11 | `baseline-system-fast` | 34 | 2109 | 1.612138% | 0.000000 | 29 | 470 |
| 11 | `baseline-best` | 22 | 2109 | 1.043148% | -0.568990 | 20 | 470 |
| 11 | `bitonal-best-vie-eng` | 22 | 2109 | 1.043148% | -0.568990 | 20 | 470 |
| 11 | `bitonal-best-vie` | 21 | 2109 | 0.995733% | -0.616406 | 17 | 470 |
| 12 | `baseline-system-fast` | 14 | 2368 | 0.591216% | 0.000000 | 14 | 532 |
| 12 | `baseline-best` | 4 | 2368 | 0.168919% | -0.422297 | 4 | 532 |
| 12 | `bitonal-best-vie-eng` | 4 | 2368 | 0.168919% | -0.422297 | 4 | 532 |
| 12 | `bitonal-best-vie` | 1 | 2368 | 0.042230% | -0.548986 | 1 | 532 |
| 13 | `baseline-system-fast` | 15 | 2738 | 0.547845% | 0.000000 | 13 | 604 |
| 13 | `baseline-best` | 7 | 2738 | 0.255661% | -0.292184 | 7 | 604 |
| 13 | `bitonal-best-vie-eng` | 7 | 2738 | 0.255661% | -0.292184 | 7 | 604 |
| 13 | `bitonal-best-vie` | 5 | 2738 | 0.182615% | -0.365230 | 5 | 604 |
| 14 | `baseline-system-fast` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `baseline-best` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `bitonal-best-vie-eng` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 14 | `bitonal-best-vie` | 6 | 2468 | 0.243112% | 0.000000 | 6 | 549 |
| 15 | `baseline-system-fast` | 19 | 2296 | 0.827526% | 0.000000 | 16 | 532 |
| 15 | `baseline-best` | 21 | 2296 | 0.914634% | 0.087108 | 19 | 532 |
| 15 | `bitonal-best-vie-eng` | 21 | 2296 | 0.914634% | 0.087108 | 19 | 532 |
| 15 | `bitonal-best-vie` | 15 | 2296 | 0.653310% | -0.174216 | 15 | 532 |
| 16 | `baseline-system-fast` | 21 | 2258 | 0.930027% | 0.000000 | 16 | 505 |
| 16 | `baseline-best` | 8 | 2258 | 0.354296% | -0.575731 | 7 | 505 |
| 16 | `bitonal-best-vie-eng` | 8 | 2258 | 0.354296% | -0.575731 | 7 | 505 |
| 16 | `bitonal-best-vie` | 3 | 2258 | 0.132861% | -0.797166 | 3 | 505 |
| 17 | `baseline-system-fast` | 24 | 2261 | 1.061477% | 0.000000 | 24 | 509 |
| 17 | `baseline-best` | 19 | 2261 | 0.840336% | -0.221141 | 19 | 509 |
| 17 | `bitonal-best-vie-eng` | 19 | 2261 | 0.840336% | -0.221141 | 19 | 509 |
| 17 | `bitonal-best-vie` | 11 | 2261 | 0.486510% | -0.574967 | 12 | 509 |
| 18 | `baseline-system-fast` | 16 | 2429 | 0.658707% | 0.000000 | 17 | 540 |
| 18 | `baseline-best` | 13 | 2429 | 0.535200% | -0.123508 | 14 | 540 |
| 18 | `bitonal-best-vie-eng` | 13 | 2429 | 0.535200% | -0.123508 | 14 | 540 |
| 18 | `bitonal-best-vie` | 10 | 2429 | 0.411692% | -0.247015 | 10 | 540 |
| 19 | `baseline-system-fast` | 20 | 2600 | 0.769231% | 0.000000 | 16 | 574 |
| 19 | `baseline-best` | 12 | 2600 | 0.461538% | -0.307692 | 10 | 574 |
| 19 | `bitonal-best-vie-eng` | 12 | 2600 | 0.461538% | -0.307692 | 10 | 574 |
| 19 | `bitonal-best-vie` | 10 | 2600 | 0.384615% | -0.384615 | 9 | 574 |
| 20 | `baseline-system-fast` | 20 | 2555 | 0.782779% | 0.000000 | 18 | 562 |
| 20 | `baseline-best` | 7 | 2555 | 0.273973% | -0.508806 | 7 | 562 |
| 20 | `bitonal-best-vie-eng` | 7 | 2555 | 0.273973% | -0.508806 | 7 | 562 |
| 20 | `bitonal-best-vie` | 7 | 2555 | 0.273973% | -0.508806 | 7 | 562 |

## Page 60 accent-error proxies

| Candidate | latin-o-for-o-with-hook | latin-u-for-u-with-hook | latin-a-for-a-with-breve |
|---|---:|---:|---:|
| `baseline-system-fast` | 0 | 25 | 0 |
| `baseline-best` | 0 | 25 | 0 |
| `bitonal-best-vie-eng` | 0 | 25 | 0 |
| `bitonal-best-vie` | 0 | 25 | 0 |

## Page 450 coverage diagnostics

| Candidate | Digit sequences | Digit-sequence SHA-256 | Legal identifiers | Non-whitespace characters | Suspicious characters |
|---|---:|---|---:|---:|---:|
| `baseline-system-fast` | 61 | `dd98c814507dd6a95350a76ecfab23c1fbb5884672d28fe7abbaf63b56ddc72e` | 0 | 1666 | 53 |
| `baseline-best` | 42 | `7aad72fb082f8de0d9049c6aba1f3fb9ca4fb35780feb8a0e440c8aa7c431d1d` | 0 | 1862 | 62 |
| `bitonal-best-vie-eng` | 42 | `7aad72fb082f8de0d9049c6aba1f3fb9ca4fb35780feb8a0e440c8aa7c431d1d` | 0 | 1862 | 62 |
| `bitonal-best-vie` | 45 | `2a84ba26fcb078ed686a58dc6516efb07f09e9a33e1e8b9dd8d8ddd8254ce97f` | 0 | 1862 | 55 |

## Candidate semantics

| Candidate | Mode | Tessdata | Languages |
|---|---|---|---|
| `baseline-system-fast` | `legacy` | `system` | `vie+eng` |
| `baseline-best` | `legacy` | `best` | `vie+eng` |
| `bitonal-best-vie-eng` | `preserve-near-bitonal` | `best` | `vie+eng` |
| `bitonal-best-vie` | `preserve-near-bitonal` | `best` | `vie` |
