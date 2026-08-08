# Vietnamese OCR tuning baseline

## Decision and scope

The baseline is accepted for tuning experiments: OCR edit counts and provenance were identical across both repetitions. Timing is measured but explicitly excluded from determinism.

**Holdout assets were not read or executed.** No holdout image was checksummed, opened, or passed to OCR in these runs. The six historical holdout pages remain untouched until the one-time Task 5 gate.

This 44-page tuning sample is provisional; **production remains blocked** because the holdout is not representative of modern documents.

Tracked artifacts contain no recognized output text. Per-page rows below contain additive edit counts only.

## Immutable provenance

| Check | SHA-256 |
| --- | --- |
| `source_sha256` | `93ce8a87d0b12ee8ef0936f5f5c20214bf26eee10e790ba3179390f8f2a103c6` |
| `split_sha256` | `092012a1b064783a221b6fe56444f26effb13c6120691dd889a78e1d0ba486d3` |
| `config_sha256` | `f8b63feb87eae3f5dc1b201f0b38a3caba7a746683fcf2b23065f20974d1b409` |
| `binary_sha256` | `d85b911c08b8c26f37995f5609f92f50ac0ec49fdacc5014ef49a6e8f8500d50` |
| `tessdata_sha256` | `{"best":{"eng":"8280aed0782fe27257a68ea10fe7ef324ca0f8d85bd2fd145d1c2b560bcb66ba","vie":"b6b49293d95d0b6dbd8780174627e82c75be957b6f4ed9862155540d6b00bb45"},"system":{"eng":"7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2","vie":"79df64caf7bcfb2a27df5042ecb6121e196eada34da774956995747636d5bfa1"}}` |
| `host_sha256` | `1ff74f6616244aeb14b84f14d0fd862ca834821406c938df597142945ba544aa` |

Host descriptor bound by `host_sha256`:

- Platform: `Linux-6.12.94+-x86_64-with-glibc2.39`
- Architecture: `x86_64`
- Logical CPUs: 8
- Memory bytes: 50538512384
- Tesseract: `tesseract 5.3.4`
- Python: `3.12.3`
- Build: `CC=gcc CXX=g++ cargo build --release -p fileconv-cli --no-default-features`

## Candidate interfaces

| Candidate | Exact argv | Environment variable names |
| --- | --- | --- |
| `markhand-default` | `["/workspace/target/release/fileconv", "one", "{input}", "--lang", "vie+eng"]` | `FILECONV_TESSDATA`, `LANG`, `LC_ALL`, `OMP_NUM_THREADS`, `OPENBLAS_NUM_THREADS`, `PATH`, `PYTHONNOUSERSITE`, `PYTHONPATH` |
| `markhand-tessdata-best` | `["/workspace/target/release/fileconv", "one", "{input}", "--lang", "vie+eng"]` | `FILECONV_TESSDATA`, `LANG`, `LC_ALL`, `OMP_NUM_THREADS`, `OPENBLAS_NUM_THREADS`, `PATH`, `PYTHONNOUSERSITE`, `PYTHONPATH` |

Only variable names are recorded; environment values are omitted.

## Aggregate measurements

| Repetition | Candidate | CER | WER | Median s/page | p95 s/page | Peak RSS MiB | Failures |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | `markhand-default` | 0.133195 | 0.304206 | 1.347830 | 2.276251 | 154.94 | 0 |
| 1 | `markhand-tessdata-best` | 0.122174 | 0.282883 | 1.827474 | 2.945982 | 175.60 | 0 |
| 2 | `markhand-default` | 0.133195 | 0.304206 | 1.349556 | 2.283584 | 154.95 | 0 |
| 2 | `markhand-tessdata-best` | 0.122174 | 0.282883 | 1.816209 | 2.924351 | 175.41 | 0 |

Timing varied as expected and was assessed separately:

| Candidate | Minimum page seconds | Maximum page seconds | Combined median page seconds |
| --- | ---: | ---: | ---: |
| `markhand-default` | 0.161876 | 2.506531 | 1.349556 |
| `markhand-tessdata-best` | 0.198616 | 3.252051 | 1.825769 |

## Per-stratum aggregates

Strata overlap by design. Values use the accepted raw counts from repetition 1; repetition 2 is count-identical.

| Candidate | Stratum | Pages | Character edits / chars | CER | Word edits / words | WER |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `markhand-default` | `clean-official` | 1 | 216 / 1112 | 0.194245 | 64 / 248 | 0.258065 |
| `markhand-default` | `dense-form` | 7 | 1509 / 12332 | 0.122365 | 570 / 2676 | 0.213004 |
| `markhand-default` | `dense-text` | 11 | 1878 / 19892 | 0.094410 | 1242 / 4388 | 0.283045 |
| `markhand-default` | `historical-old-print` | 35 | 5372 / 39759 | 0.135114 | 2916 / 8671 | 0.336293 |
| `markhand-default` | `low-contrast` | 8 | 1868 / 10002 | 0.186763 | 930 / 2224 | 0.418165 |
| `markhand-default` | `modern-government` | 9 | 1940 / 15138 | 0.128154 | 722 / 3288 | 0.219586 |
| `markhand-default` | `skew` | 6 | 741 / 6596 | 0.112341 | 406 / 1414 | 0.287129 |
| `markhand-default` | `small-text` | 12 | 2141 / 22375 | 0.095687 | 1326 / 4923 | 0.269348 |
| `markhand-default` | `stamp-watermark` | 7 | 1461 / 11543 | 0.126570 | 574 / 2505 | 0.229142 |
| `markhand-tessdata-best` | `clean-official` | 1 | 209 / 1112 | 0.187950 | 60 / 248 | 0.241935 |
| `markhand-tessdata-best` | `dense-form` | 7 | 1397 / 12332 | 0.113283 | 507 / 2676 | 0.189462 |
| `markhand-tessdata-best` | `dense-text` | 11 | 1677 / 19892 | 0.084305 | 1158 / 4388 | 0.263902 |
| `markhand-tessdata-best` | `historical-old-print` | 35 | 4910 / 39759 | 0.123494 | 2744 / 8671 | 0.316457 |
| `markhand-tessdata-best` | `low-contrast` | 8 | 1640 / 10002 | 0.163967 | 844 / 2224 | 0.379496 |
| `markhand-tessdata-best` | `modern-government` | 9 | 1797 / 15138 | 0.118708 | 639 / 3288 | 0.194343 |
| `markhand-tessdata-best` | `skew` | 6 | 701 / 6596 | 0.106277 | 393 / 1414 | 0.277935 |
| `markhand-tessdata-best` | `small-text` | 12 | 1934 / 22375 | 0.086436 | 1238 / 4923 | 0.251473 |
| `markhand-tessdata-best` | `stamp-watermark` | 7 | 1331 / 11543 | 0.115308 | 499 / 2505 | 0.199202 |

## Raw additive counts

These rows are sufficient to recompute every overall and overlapping-stratum micro-average. CER is total character edits divided by total chars; WER is total word edits divided by total words.

| Candidate | Page ID | Difficulty strata | Character edits | Chars | Word edits | Words |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| `markhand-default` | nrl-ai:real_cv_3722_metro:1 | `dense-form`, `modern-government`, `stamp-watermark` | 279 | 1485 | 94 | 329 |
| `markhand-default` | nrl-ai:real_hanoi_kh_173_festival:1 | `dense-form`, `modern-government`, `small-text` | 263 | 2483 | 84 | 535 |
| `markhand-default` | nrl-ai:real_hanoi_qd_2280_water:1 | `low-contrast`, `modern-government`, `stamp-watermark` | 215 | 1694 | 88 | 364 |
| `markhand-default` | nrl-ai:real_hanoi_tb_453_flag:1 | `clean-official`, `modern-government` | 216 | 1112 | 64 | 248 |
| `markhand-default` | nrl-ai:real_nq_115_ba_vi:1 | `dense-form`, `modern-government`, `stamp-watermark` | 177 | 1861 | 64 | 402 |
| `markhand-default` | nrl-ai:real_qd_707_ttg:1 | `dense-form`, `modern-government`, `stamp-watermark` | 100 | 1285 | 48 | 284 |
| `markhand-default` | nrl-ai:real_qd_729_ttg:1 | `dense-form`, `modern-government`, `stamp-watermark` | 266 | 1608 | 82 | 356 |
| `markhand-default` | nrl-ai:real_tt_21_bct_fuel:1 | `dense-form`, `modern-government`, `stamp-watermark` | 201 | 1661 | 90 | 354 |
| `markhand-default` | nrl-ai:real_tt_37_bca_vehicle:1 | `dense-form`, `modern-government`, `stamp-watermark` | 223 | 1949 | 108 | 416 |
| `markhand-default` | wikisource:Bai dien thuyet cua cu Phan Boi Chau ngay 17 Mars 1926.pdf:0007 | `dense-text`, `historical-old-print`, `skew`, `small-text` | 216 | 1779 | 103 | 401 |
| `markhand-default` | wikisource:Bai dien thuyet cua cu Phan Boi Chau ngay 17 Mars 1926.pdf:0010 | `historical-old-print`, `skew` | 130 | 1313 | 54 | 293 |
| `markhand-default` | wikisource:Cay dang mui doi 1.pdf:0024 | `dense-text`, `historical-old-print`, `small-text` | 170 | 2024 | 140 | 481 |
| `markhand-default` | wikisource:Cay dang mui doi 1.pdf:0027 | `dense-text`, `historical-old-print`, `small-text` | 185 | 1868 | 150 | 446 |
| `markhand-default` | wikisource:Cay dang mui doi 2.pdf:0011 | `dense-text`, `historical-old-print`, `small-text` | 131 | 1928 | 104 | 460 |
| `markhand-default` | wikisource:Cay dang mui doi 2.pdf:0012 | `dense-text`, `historical-old-print`, `small-text` | 147 | 1689 | 95 | 390 |
| `markhand-default` | wikisource:Chu nho hoc lay 1.pdf:0009 | `historical-old-print` | 237 | 1159 | 97 | 267 |
| `markhand-default` | wikisource:Chu nho hoc lay 1.pdf:0020 | `historical-old-print` | 119 | 661 | 70 | 148 |
| `markhand-default` | wikisource:Chuyen the gian 1.pdf:0014 | `historical-old-print` | 103 | 1211 | 64 | 255 |
| `markhand-default` | wikisource:Chuyen the gian 1.pdf:0026 | `historical-old-print` | 97 | 1450 | 68 | 311 |
| `markhand-default` | wikisource:Co xuy nguyen am.pdf:0010 | `historical-old-print`, `low-contrast` | 510 | 939 | 218 | 269 |
| `markhand-default` | wikisource:Co xuy nguyen am.pdf:0015 | `dense-text`, `historical-old-print`, `low-contrast`, `small-text` | 315 | 1539 | 165 | 321 |
| `markhand-default` | wikisource:Co xuy nguyen am.pdf:0019 | `historical-old-print`, `low-contrast` | 458 | 1277 | 194 | 283 |
| `markhand-default` | wikisource:Giac mong con 1926.pdf:0001 | `historical-old-print` | 78 | 88 | 12 | 12 |
| `markhand-default` | wikisource:Giac mong con 1926.pdf:0021 | `dense-text`, `historical-old-print`, `small-text` | 91 | 1517 | 78 | 315 |
| `markhand-default` | wikisource:Gương sử Nam - Hoàng Thái-Xuyên (1910).pdf:0007 | `historical-old-print`, `low-contrast` | 114 | 1430 | 83 | 283 |
| `markhand-default` | wikisource:Gương sử Nam - Hoàng Thái-Xuyên (1910).pdf:0023 | `historical-old-print`, `low-contrast` | 148 | 1491 | 112 | 320 |
| `markhand-default` | wikisource:Tam quoc Nguyen An Cu 1928 - 01.pdf:0008 | `dense-text`, `historical-old-print`, `small-text` | 204 | 2238 | 128 | 468 |
| `markhand-default` | wikisource:Tam quoc Nguyen An Cu 1928 - 01.pdf:0021 | `dense-text`, `historical-old-print`, `small-text` | 167 | 1718 | 118 | 321 |
| `markhand-default` | wikisource:Tho Tan Da.pdf:0001 | `historical-old-print`, `skew` | 78 | 92 | 15 | 15 |
| `markhand-default` | wikisource:Tho Tan Da.pdf:0029 | `historical-old-print`, `skew` | 127 | 855 | 101 | 180 |
| `markhand-default` | wikisource:Tho ngu ngon La Fontaine Nguyen Van Vinh 1951.pdf:0011 | `historical-old-print` | 151 | 739 | 62 | 161 |
| `markhand-default` | wikisource:Tho ngu ngon La Fontaine Nguyen Van Vinh 1951.pdf:0019 | `historical-old-print` | 161 | 770 | 74 | 166 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 1.pdf:0090 | `historical-old-print` | 123 | 489 | 50 | 79 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 3.pdf:0001 | `historical-old-print` | 92 | 112 | 21 | 17 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 4.pdf:0041 | `historical-old-print` | 96 | 1177 | 58 | 252 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 4.pdf:0043 | `historical-old-print` | 66 | 1166 | 63 | 243 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 5.pdf:0016 | `historical-old-print` | 47 | 1184 | 38 | 236 |
| `markhand-default` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 5.pdf:0020 | `historical-old-print` | 109 | 1263 | 63 | 254 |
| `markhand-default` | wikisource:Van de phu nu.pdf:0006 | `historical-old-print`, `skew` | 45 | 579 | 41 | 122 |
| `markhand-default` | wikisource:Van de phu nu.pdf:0014 | `dense-text`, `historical-old-print`, `skew`, `small-text` | 145 | 1978 | 92 | 403 |
| `markhand-default` | wikisource:Viet Nam Su Luoc, Quyen 2, 1928.pdf:0002 | `historical-old-print` | 187 | 187 | 32 | 32 |
| `markhand-default` | wikisource:Viet Nam Su Luoc, Quyen 2, 1928.pdf:0030 | `historical-old-print` | 217 | 217 | 83 | 83 |
| `markhand-default` | wikisource:Viet Nam phong tuc.pdf:0002 | `historical-old-print`, `low-contrast` | 1 | 18 | 1 | 2 |
| `markhand-default` | wikisource:Viet Nam phong tuc.pdf:0010 | `dense-text`, `historical-old-print`, `low-contrast`, `small-text` | 107 | 1614 | 69 | 382 |
| `markhand-tessdata-best` | nrl-ai:real_cv_3722_metro:1 | `dense-form`, `modern-government`, `stamp-watermark` | 255 | 1485 | 82 | 329 |
| `markhand-tessdata-best` | nrl-ai:real_hanoi_kh_173_festival:1 | `dense-form`, `modern-government`, `small-text` | 257 | 2483 | 80 | 535 |
| `markhand-tessdata-best` | nrl-ai:real_hanoi_qd_2280_water:1 | `low-contrast`, `modern-government`, `stamp-watermark` | 191 | 1694 | 72 | 364 |
| `markhand-tessdata-best` | nrl-ai:real_hanoi_tb_453_flag:1 | `clean-official`, `modern-government` | 209 | 1112 | 60 | 248 |
| `markhand-tessdata-best` | nrl-ai:real_nq_115_ba_vi:1 | `dense-form`, `modern-government`, `stamp-watermark` | 169 | 1861 | 55 | 402 |
| `markhand-tessdata-best` | nrl-ai:real_qd_707_ttg:1 | `dense-form`, `modern-government`, `stamp-watermark` | 94 | 1285 | 41 | 284 |
| `markhand-tessdata-best` | nrl-ai:real_qd_729_ttg:1 | `dense-form`, `modern-government`, `stamp-watermark` | 242 | 1608 | 70 | 356 |
| `markhand-tessdata-best` | nrl-ai:real_tt_21_bct_fuel:1 | `dense-form`, `modern-government`, `stamp-watermark` | 171 | 1661 | 81 | 354 |
| `markhand-tessdata-best` | nrl-ai:real_tt_37_bca_vehicle:1 | `dense-form`, `modern-government`, `stamp-watermark` | 209 | 1949 | 98 | 416 |
| `markhand-tessdata-best` | wikisource:Bai dien thuyet cua cu Phan Boi Chau ngay 17 Mars 1926.pdf:0007 | `dense-text`, `historical-old-print`, `skew`, `small-text` | 201 | 1779 | 98 | 401 |
| `markhand-tessdata-best` | wikisource:Bai dien thuyet cua cu Phan Boi Chau ngay 17 Mars 1926.pdf:0010 | `historical-old-print`, `skew` | 115 | 1313 | 51 | 293 |
| `markhand-tessdata-best` | wikisource:Cay dang mui doi 1.pdf:0024 | `dense-text`, `historical-old-print`, `small-text` | 145 | 2024 | 125 | 481 |
| `markhand-tessdata-best` | wikisource:Cay dang mui doi 1.pdf:0027 | `dense-text`, `historical-old-print`, `small-text` | 178 | 1868 | 147 | 446 |
| `markhand-tessdata-best` | wikisource:Cay dang mui doi 2.pdf:0011 | `dense-text`, `historical-old-print`, `small-text` | 113 | 1928 | 88 | 460 |
| `markhand-tessdata-best` | wikisource:Cay dang mui doi 2.pdf:0012 | `dense-text`, `historical-old-print`, `small-text` | 149 | 1689 | 94 | 390 |
| `markhand-tessdata-best` | wikisource:Chu nho hoc lay 1.pdf:0009 | `historical-old-print` | 235 | 1159 | 98 | 267 |
| `markhand-tessdata-best` | wikisource:Chu nho hoc lay 1.pdf:0020 | `historical-old-print` | 114 | 661 | 76 | 148 |
| `markhand-tessdata-best` | wikisource:Chuyen the gian 1.pdf:0014 | `historical-old-print` | 112 | 1211 | 79 | 255 |
| `markhand-tessdata-best` | wikisource:Chuyen the gian 1.pdf:0026 | `historical-old-print` | 112 | 1450 | 69 | 311 |
| `markhand-tessdata-best` | wikisource:Co xuy nguyen am.pdf:0010 | `historical-old-print`, `low-contrast` | 406 | 939 | 193 | 269 |
| `markhand-tessdata-best` | wikisource:Co xuy nguyen am.pdf:0015 | `dense-text`, `historical-old-print`, `low-contrast`, `small-text` | 313 | 1539 | 173 | 321 |
| `markhand-tessdata-best` | wikisource:Co xuy nguyen am.pdf:0019 | `historical-old-print`, `low-contrast` | 416 | 1277 | 179 | 283 |
| `markhand-tessdata-best` | wikisource:Giac mong con 1926.pdf:0001 | `historical-old-print` | 79 | 88 | 12 | 12 |
| `markhand-tessdata-best` | wikisource:Giac mong con 1926.pdf:0021 | `dense-text`, `historical-old-print`, `small-text` | 71 | 1517 | 71 | 315 |
| `markhand-tessdata-best` | wikisource:Gương sử Nam - Hoàng Thái-Xuyên (1910).pdf:0007 | `historical-old-print`, `low-contrast` | 98 | 1430 | 69 | 283 |
| `markhand-tessdata-best` | wikisource:Gương sử Nam - Hoàng Thái-Xuyên (1910).pdf:0023 | `historical-old-print`, `low-contrast` | 134 | 1491 | 104 | 320 |
| `markhand-tessdata-best` | wikisource:Tam quoc Nguyen An Cu 1928 - 01.pdf:0008 | `dense-text`, `historical-old-print`, `small-text` | 165 | 2238 | 112 | 468 |
| `markhand-tessdata-best` | wikisource:Tam quoc Nguyen An Cu 1928 - 01.pdf:0021 | `dense-text`, `historical-old-print`, `small-text` | 130 | 1718 | 106 | 321 |
| `markhand-tessdata-best` | wikisource:Tho Tan Da.pdf:0001 | `historical-old-print`, `skew` | 79 | 92 | 15 | 15 |
| `markhand-tessdata-best` | wikisource:Tho Tan Da.pdf:0029 | `historical-old-print`, `skew` | 129 | 855 | 94 | 180 |
| `markhand-tessdata-best` | wikisource:Tho ngu ngon La Fontaine Nguyen Van Vinh 1951.pdf:0011 | `historical-old-print` | 127 | 739 | 54 | 161 |
| `markhand-tessdata-best` | wikisource:Tho ngu ngon La Fontaine Nguyen Van Vinh 1951.pdf:0019 | `historical-old-print` | 139 | 770 | 63 | 166 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 1.pdf:0090 | `historical-old-print` | 121 | 489 | 46 | 79 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 3.pdf:0001 | `historical-old-print` | 85 | 112 | 22 | 17 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 4.pdf:0041 | `historical-old-print` | 86 | 1177 | 49 | 252 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 4.pdf:0043 | `historical-old-print` | 53 | 1166 | 56 | 243 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 5.pdf:0016 | `historical-old-print` | 39 | 1184 | 34 | 236 |
| `markhand-tessdata-best` | wikisource:Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 5.pdf:0020 | `historical-old-print` | 103 | 1263 | 63 | 254 |
| `markhand-tessdata-best` | wikisource:Van de phu nu.pdf:0006 | `historical-old-print`, `skew` | 46 | 579 | 44 | 122 |
| `markhand-tessdata-best` | wikisource:Van de phu nu.pdf:0014 | `dense-text`, `historical-old-print`, `skew`, `small-text` | 131 | 1978 | 91 | 403 |
| `markhand-tessdata-best` | wikisource:Viet Nam Su Luoc, Quyen 2, 1928.pdf:0002 | `historical-old-print` | 187 | 187 | 32 | 32 |
| `markhand-tessdata-best` | wikisource:Viet Nam Su Luoc, Quyen 2, 1928.pdf:0030 | `historical-old-print` | 217 | 217 | 83 | 83 |
| `markhand-tessdata-best` | wikisource:Viet Nam phong tuc.pdf:0002 | `historical-old-print`, `low-contrast` | 1 | 18 | 1 | 2 |
| `markhand-tessdata-best` | wikisource:Viet Nam phong tuc.pdf:0010 | `dense-text`, `historical-old-print`, `low-contrast`, `small-text` | 81 | 1614 | 53 | 382 |

Overall recomputation:
- `markhand-default` CER: 7312 / 54897 = 0.133195; WER: 3638 / 11959 = 0.304206.
- `markhand-tessdata-best` CER: 6707 / 54897 = 0.122174; WER: 3383 / 11959 = 0.282883.

## Bounded execution semantics

- Candidates ran serially, with one warm benchmark worker at a time and one fileconv process per page.
- Each page had a 180-second wall deadline. Timeout cleanup terminates the complete process group.
- Candidate stdout and stderr each had a hard 1,048,576-byte bounded-pipe collection limit; overflow terminates the process tree.
- RSS is a 10 ms sampled process-tree sum. The 4 GiB threshold is a measured gate, not an OS-enforced memory limit.
- Page latency spans warm worker request through result and includes the fileconv subprocess execution. Cold worker initialization is recorded separately in local run artifacts.
- At most eight worst-CER page diagnostics per candidate are retained in local artifacts; diagnostics and tracked tables contain no OCR text.

## Determinism

The configured OCR-count tolerance is zero. Candidate order, argv, environment-variable names, source/split/config/binary/tessdata/host checksums, per-page success state, and every additive edit count were identical. Latency and RSS variance did not participate in acceptance.
