# Frozen Vietnamese real-scan accuracy corpus

## Decision

This freeze contains exactly 50 real scan pages with human-supplied reference
text: 44 tuning pages and 6 holdout pages. It is suitable for the planned
bounded OCR experiments, but it is **provisional and not production-
representative**. The holdout is entirely historical print. Production adoption
is blocked until the holdout adds 9–14 modern pages whose labels have received
independent human review.

The freeze digest is
`7f1e56be8799bfee6f7c60c90babc45d86bc706c41089c0ced3c9b15392fe309`.
The ignored local image set contains 50 files totaling 32,757,918 bytes.

## Composition

| Split | Modern government | Historical books | Pages | Families |
| --- | ---: | ---: | ---: | ---: |
| tuning | 9 | 35 | 44 | 27 |
| holdout | 0 | 6 | 6 | 3 |
| total | 9 | 41 | 50 | 30 |

The 35 historical tuning pages are Proofread pages balanced over 18 source
books: 16 books contribute two pages, one contributes three, and two contribute
one. None is represented in holdout. The nine modern pages are the existing
`nrl-ai/vn-ocr-documents-eval` public-domain government scans with the
dataset-declared human-verified labels.

Holdout is all six Validated rows found in the 355-row Latin Quốc ngữ
Wikisource test subset:

| Source work | Frozen pages (revision ID) |
| --- | --- |
| `Cung oan ngam khuc 1905.pdf` | 1 (`72175`), 15 (`81597`), 16 (`81598`) |
| `Kinh Thanh Cuu Uoc Va Tan Uoc 1925.pdf` | 1087 (`200797`), 1110 (`202078`) |
| `Tan Da tung van.pdf` | 10 (`156337`) |

Every other row from those three works is excluded from tuning. No source or
family crosses splits.

Tuning source counts are:

| Source family | Pages |
| --- | ---: |
| `Bai dien thuyet cua cu Phan Boi Chau ngay 17 Mars 1926.pdf` | 2 |
| `Cay dang mui doi 1.pdf` | 2 |
| `Cay dang mui doi 2.pdf` | 2 |
| `Chu nho hoc lay 1.pdf` | 2 |
| `Chuyen the gian 1.pdf` | 2 |
| `Co xuy nguyen am.pdf` | 3 |
| `Giac mong con 1926.pdf` | 2 |
| `Gương sử Nam - Hoàng Thái-Xuyên (1910).pdf` | 2 |
| `Tam quoc Nguyen An Cu 1928 - 01.pdf` | 2 |
| `Tho Tan Da.pdf` | 2 |
| `Tho ngu ngon La Fontaine Nguyen Van Vinh 1951.pdf` | 2 |
| `Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 1.pdf` | 1 |
| `Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 3.pdf` | 1 |
| `Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 4.pdf` | 2 |
| `Tân Dân Tử, Gia Long Tẩu Quốc, Quyển 5.pdf` | 2 |
| `Van de phu nu.pdf` | 2 |
| `Viet Nam Su Luoc, Quyen 2, 1928.pdf` | 2 |
| `Viet Nam phong tuc.pdf` | 2 |
| nine distinct modern government documents | 1 each |

Curator-assigned qualitative strata overlap. Counts are: historical old print
41, small text 13, dense text 12, modern government 9, low contrast 8, dense
form 7, stamp/watermark 7, skew 6, and clean official 1.

## Provenance and review semantics

A live ProofreadPage audit of all 355 `wikisource_qn` test rows found 286
Proofread, 6 Validated, and 63 unproofread/problematic pages. Only Proofread and
Validated rows are admissible. Each selected Wikisource annotation records the
direct `Trang:` URL, exact current revision ID, status, quality-user evidence,
source family, page number, label, and image/package checksums.

Proofread means one contributor marked the page Proofread; it is human
proofreading evidence but **not independent validation**. Accordingly,
Proofread rows have no reviewer claim. Validated rows record separately the
quality user from the earlier Proofread revision and the quality user from the
Validated revision. Labels are the dataset's Wikisource-derived human text,
never OCR or PDF-extracted text.

MeddiesOCR, PNTV, all unproofread rows, and all non-`wikisource_qn` Vietage
groups are excluded.

## Licensing

The nine modern images and labels are distributed by the nrl-ai dataset under
CC0-1.0; their source records identify Vietnamese government documents as
public domain under Vietnam Intellectual Property Law Article 15.

The 41 selected Wikisource rows identify their page text as CC-BY-SA-4.0 and
the underlying historical scans as public-domain works. However, the frozen
Hugging Face package `taidng/vietage-ocr` is distributed as
**CC-BY-NC-SA-4.0**. Use of assets acquired through that package retains those
dataset-level non-commercial/share-alike implications. This report does not
relicense the package or imply that the page-level license removes its terms.

Only permitted annotation text, provenance metadata, and checksums are tracked.
The 470,399,464-byte pinned Parquet package and selected images stay under the
ignored `.data/corpus` tree.

## Reproduction and validation

`accuracy-sources.json` pins the nrl-ai assets and Vietage package by immutable
commit URLs and SHA-256. Acquisition uses the existing downloader's HTTPS-only
allowlist, public-address DNS validation, hostname-verified TLS, per-redirect
revalidation, byte limits, and a bounded total deadline.

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m corpus.split \
  bench/ocr_cpu_service/corpus/accuracy-annotations.jsonl \
  --assets bench/ocr_cpu_service/.data/corpus
```

Two consecutive runs returned the same digest, page count, asset count, and
byte count. The schema loader fails closed on missing fields, non-human labels,
bad checksums, duplicate source/page keys, and family leakage.
