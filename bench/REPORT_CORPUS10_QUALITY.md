# Markhand CORPUS10 — quality report

- Files: **90** public internet samples.
- Successful conversions: **90/90**.
- Path: desktop-equivalent release `fileconv_core::Converter`.
- Audio model: Whisper tiny; samples are decoder/music fixtures, not Vietnamese WER ground truth.

## Summary

| Family | N | OK | Non-empty | Median ms | Median chars | Errors | Warnings |
|---|--:|--:|--:|--:|--:|--:|--:|
| audio | 10 | 10 | 1 | 759.96 | 0 | 0 | 0 |
| csv | 10 | 10 | 10 | 2.47 | 4032 | 0 | 0 |
| docx | 10 | 10 | 8 | 2.75 | 46 | 0 | 1 |
| html | 10 | 10 | 10 | 4.16 | 32515 | 0 | 0 |
| image | 10 | 10 | 10 | 476.99 | 174 | 0 | 0 |
| pdf | 10 | 10 | 10 | 79.46 | 45908 | 0 | 0 |
| pptx | 10 | 10 | 4 | 2.08 | 0 | 0 | 0 |
| spreadsheet | 10 | 10 | 9 | 2.06 | 129 | 0 | 1 |
| text | 10 | 10 | 10 | 3.96 | 598326 | 0 | 0 |

## Per-file

| Family | File | ms | Chars | Headings | Table rows | Assessment |
|---|---|--:|--:|--:|--:|---|
| audio | `lena.flac` | 2554.84 | 0 | 0 | 0 | pass |
| audio | `lena.m4a` | 553.97 | 0 | 0 | 0 | pass |
| audio | `lena.ogg` | 2526.69 | 0 | 0 | 0 | pass |
| audio | `sample-12s.mp3` | 549.59 | 0 | 0 | 0 | pass |
| audio | `sample-15s.mp3` | 564.17 | 0 | 0 | 0 | pass |
| audio | `sample-3s.mp3` | 963.37 | 0 | 0 | 0 | pass |
| audio | `sample-3s.wav` | 1452.78 | 4 | 0 | 0 | info: non-empty transcript; codec fixture has no ground-truth speech |
| audio | `sample-6s.mp3` | 955.74 | 0 | 0 | 0 | pass |
| audio | `sample-6s.wav` | 538.02 | 0 | 0 | 0 | pass |
| audio | `sample-9s.mp3` | 546.79 | 0 | 0 | 0 | pass |
| csv | `airtravel.csv` | 2.47 | 374 | 0 | 14 | pass |
| csv | `biostats.csv` | 2.69 | 646 | 0 | 20 | pass |
| csv | `cities.csv` | 2.48 | 8176 | 0 | 130 | pass |
| csv | `deniro.csv` | 2.17 | 3108 | 0 | 89 | pass |
| csv | `faithful.csv` | 2.53 | 5683 | 0 | 274 | pass |
| csv | `grades.csv` | 2.25 | 1420 | 0 | 18 | pass |
| csv | `homes.csv` | 2.19 | 2667 | 0 | 53 | pass |
| csv | `hw_200.csv` | 2.29 | 4956 | 0 | 202 | pass |
| csv | `mlb_players.csv` | 3.15 | 64191 | 0 | 1037 | pass |
| csv | `snakes_count_10000.csv` | 6.24 | 139027 | 0 | 10002 | pass |
| docx | `blk-containing-table.docx` | 2.78 | 29 | 0 | 3 | pass |
| docx | `blk-paras-and-tables.docx` | 3.03 | 214 | 0 | 5 | pass |
| docx | `doc-coreprops.docx` | 2.75 | 0 | 0 | 0 | pass |
| docx | `doc-default.docx` | 2.41 | 0 | 0 | 0 | warning: non-trivial DOCX produced no text |
| docx | `doc-odd-even-hdrs.docx` | 2.84 | 25 | 0 | 0 | pass |
| docx | `hdr-header-footer.docx` | 2.86 | 55 | 0 | 0 | pass |
| docx | `par-alignment.docx` | 2.72 | 85 | 0 | 0 | pass |
| docx | `par-hyperlinks.docx` | 2.64 | 101 | 0 | 0 | pass |
| docx | `par-known-paragraphs.docx` | 2.71 | 37 | 1 | 0 | pass |
| docx | `run-char-style.docx` | 2.74 | 125 | 0 | 0 | pass |
| html | `example.html` | 1.94 | 183 | 1 | 0 | pass |
| html | `gpl3.html` | 3.10 | 39387 | 23 | 0 | pass |
| html | `iana-example-domains.html` | 2.13 | 1835 | 2 | 0 | pass |
| html | `python-tutorial.html` | 2.96 | 15148 | 9 | 0 | pass |
| html | `rust-book.html` | 2.29 | 1697 | 3 | 0 | pass |
| html | `w3c-png.html` | 20.04 | 357999 | 185 | 301 | pass |
| html | `wiki-markdown-vi.html` | 5.22 | 25643 | 5 | 12 | pass |
| html | `wiki-markdown.html` | 9.75 | 87143 | 14 | 48 | pass |
| html | `wiki-rust.html` | 26.23 | 269842 | 58 | 52 | pass |
| html | `wiki-vietnam-vi.html` | 58.67 | 811154 | 44 | 119 | pass |
| image | `jfif-300-dpi.jpg` | 1566.45 | 379 | 0 | 0 | pass |
| image | `monty-truth.png` | 371.91 | 18 | 0 | 0 | pass |
| image | `python-powered.png` | 165.13 | 17 | 0 | 0 | pass |
| image | `sample.tif` | 869.38 | 29 | 0 | 0 | pass |
| image | `tesseract-2col.png` | 1132.80 | 1321 | 0 | 0 | pass |
| image | `tesseract-bilingual.png` | 198.56 | 16 | 0 | 0 | pass |
| image | `tesseract-eurotext.png` | 646.03 | 413 | 0 | 0 | pass |
| image | `tesseract-phototest.tif` | 476.42 | 285 | 0 | 0 | pass |
| image | `tesseract-toc.png` | 477.57 | 543 | 0 | 0 | pass |
| image | `test.png` | 393.05 | 64 | 0 | 0 | pass |
| pdf | `arxiv-1409.1556.pdf` | 59.91 | 54705 | 18 | 10 | pass |
| pdf | `arxiv-1412.6980.pdf` | 61.43 | 42819 | 21 | 61 | pass |
| pdf | `arxiv-1505.04597.pdf` | 29.14 | 19890 | 9 | 15 | pass |
| pdf | `arxiv-1506.02640.pdf` | 50.78 | 42742 | 18 | 9 | pass |
| pdf | `arxiv-1512.03385.pdf` | 63.56 | 59749 | 15 | 15 | pass |
| pdf | `arxiv-1706.03762.pdf` | 127.01 | 39892 | 19 | 20 | pass |
| pdf | `arxiv-1707.06347.pdf` | 287.23 | 27703 | 18 | 12 | pass |
| pdf | `arxiv-1810.04805.pdf` | 105.60 | 65376 | 14 | 87 | pass |
| pdf | `arxiv-1810.13243.pdf` | 95.36 | 48998 | 14 | 11 | pass |
| pdf | `arxiv-2010.11929.pdf` | 114.76 | 66991 | 12 | 72 | pass |
| pptx | `shp-access-chart.pptx` | 2.00 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| pptx | `shp-autoshape-props.pptx` | 2.18 | 50 | 1 | 0 | pass |
| pptx | `shp-common-props.pptx` | 2.14 | 52 | 2 | 0 | pass |
| pptx | `shp-connector-props.pptx` | 2.05 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| pptx | `shp-freeform.pptx` | 2.07 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| pptx | `shp-groupshape.pptx` | 2.09 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| pptx | `shp-picture.pptx` | 2.02 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| pptx | `shp-pos-and-size.pptx` | 2.12 | 86 | 1 | 0 | pass |
| pptx | `shp-shapes.pptx` | 2.18 | 62 | 1 | 0 | pass |
| pptx | `sld-blank.pptx` | 2.07 | 0 | 0 | 0 | info: shape/image-only deck; inspect visual preview |
| spreadsheet | `any_sheets.xlsb` | 2.18 | 141 | 1 | 6 | pass |
| spreadsheet | `any_sheets.xlsx` | 2.06 | 141 | 1 | 6 | pass |
| spreadsheet | `date.ods` | 2.06 | 122 | 1 | 5 | pass |
| spreadsheet | `date.xls` | 1.90 | 82 | 1 | 4 | pass |
| spreadsheet | `date.xlsb` | 2.28 | 80 | 1 | 4 | pass |
| spreadsheet | `inventory-table.xlsx` | 2.05 | 136 | 1 | 6 | pass |
| spreadsheet | `issue127.xls` | 1.98 | 0 | 0 | 0 | warning: spreadsheet produced no cells |
| spreadsheet | `merged_cells.ods` | 2.00 | 70 | 1 | 4 | pass |
| spreadsheet | `merged_range.xls` | 1.86 | 709 | 2 | 0 | pass |
| spreadsheet | `merged_range.xlsx` | 2.23 | 709 | 2 | 0 | pass |
| text | `gutenberg-11.txt` | 2.58 | 167706 | 0 | 0 | pass |
| text | `gutenberg-1342.txt` | 3.95 | 763076 | 0 | 0 | pass |
| text | `gutenberg-1661.txt` | 3.97 | 593905 | 0 | 0 | pass |
| text | `gutenberg-2701.txt` | 5.74 | 1260587 | 0 | 0 | pass |
| text | `gutenberg-345.txt` | 4.03 | 881054 | 0 | 0 | pass |
| text | `gutenberg-5200.txt` | 2.75 | 140482 | 0 | 0 | pass |
| text | `gutenberg-74.txt` | 3.56 | 421354 | 0 | 0 | pass |
| text | `gutenberg-76.txt` | 4.65 | 602746 | 0 | 0 | pass |
| text | `gutenberg-84.txt` | 3.44 | 446576 | 0 | 0 | pass |
| text | `gutenberg-98.txt` | 4.84 | 793189 | 0 | 0 | pass |

## PPTX visual preview

| File | Slides | Rendered shapes | Text shapes | Images | Result |
|---|--:|--:|--:|--:|---|
| `shp-access-chart.pptx` | 2 | 2 | 0 | 0 | pass |
| `shp-autoshape-props.pptx` | 1 | 1 | 1 | 0 | pass |
| `shp-common-props.pptx` | 2 | 18 | 3 | 2 | pass |
| `shp-connector-props.pptx` | 2 | 3 | 0 | 1 | pass |
| `shp-freeform.pptx` | 1 | 0 | 0 | 0 | pass |
| `shp-groupshape.pptx` | 1 | 4 | 0 | 0 | pass |
| `shp-picture.pptx` | 2 | 4 | 0 | 4 | pass |
| `shp-pos-and-size.pptx` | 3 | 3 | 1 | 1 | pass |
| `shp-shapes.pptx` | 2 | 12 | 3 | 2 | pass |
| `sld-blank.pptx` | 1 | 0 | 0 | 0 | pass |

## Interpretation limits

- Public format fixtures validate compatibility and structure, not Vietnamese semantic accuracy.
- Image files mix OCR documents and decorative assets; empty decorative-image output is not a failure.
- Audio samples are mostly music/codec fixtures; short or empty output is preferred over hallucinated speech.
- BRD/PRD, citation grounding and Vietnamese OCR accuracy remain covered by their dedicated manifests.
