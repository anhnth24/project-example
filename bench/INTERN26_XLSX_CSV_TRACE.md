# Issue #364 — XLSX/CSV table detection trace (intern-26)

> Read-only trace trên fixture local (copy từ `bench/sample10/` → `/tmp/issue-364-fixtures/` khi chạy).
> Ngày: 11/08/2026.

## Files traced

| Module | Role |
|--------|------|
| [`crates/core/src/conv/xlsx.rs`](../crates/core/src/conv/xlsx.rs) | calamine, all sheets, merge → HTML branch |
| [`crates/core/src/conv/csv_conv.rs`](../crates/core/src/conv/csv_conv.rs) | sniff delimiter, `rows_to_md_table`, `rows_to_html_table` |
| [`crates/core/src/conv/mod.rs`](../crates/core/src/conv/mod.rs) | `esc_cell` |

## Fixtures & commands

```bash
mkdir -p /tmp/issue-364-fixtures
cp bench/sample10/xlsx/{28iterators,32chartreadwrite,26template}.xlsx /tmp/issue-364-fixtures/
cp bench/sample10/csv/airtravel.csv /tmp/issue-364-fixtures/

./target/release/fileconv one /tmp/issue-364-fixtures/28iterators.xlsx
./target/release/fileconv one /tmp/issue-364-fixtures/32chartreadwrite.xlsx
./target/release/fileconv one /tmp/issue-364-fixtures/26template.xlsx
./target/release/fileconv one tests/fixtures/sample/contacts.vi.csv
./target/release/fileconv one /tmp/issue-364-fixtures/28iterators.xlsx --sheet Sheet2
```

## Findings

### Header

Hàng **0** = header (Markdown `| --- |` hoặc HTML `<th>`). Không có heuristic “dòng nào là tiêu đề”.

### Merged cells

- calamine `worksheet_merge_cells` (xlsx/xls only) → `MergeRange` → `colspan`/`rowspan`
- **26template:** `<th colspan="3">` title merge
- **32chartreadwrite Data:** nested `colspan` (Financial Period / years / quarters)

### Multi-sheet

- Mỗi sheet: `## {name}` + bảng riêng; merge **không** xuyên sheet
- Sheet rỗng (Charts trong 32chartreadwrite) → skip
- `--sheet Sheet2` → chỉ Sheet2

### CSV

- Delimiter sniff: `contacts.vi.csv` → `;`
- `esc_cell`: newline trong quoted field → space (`multiline-field.csv`: `Line one Line two`)
- CSV luôn Markdown (không HTML)

## Markdown vs HTML

| Condition | Output |
|-----------|--------|
| Plain grid, no merge | Markdown `\|...\|` |
| XLSX merge or multiline cell | HTML `<table>` |
| CSV | Markdown only |

Closes https://github.com/anhnth24/project-example/issues/364
