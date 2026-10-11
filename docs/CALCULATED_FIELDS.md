# Calculated fields and imports

Add a field with type **Calculated** in the Fields view. The guided builder starts with **Multiply values**. Choose a calculation and fill its named inputs using typed field pickers or fixed values. Common tasks include adding values, percentages, combining text, rounding, and if/otherwise comparisons. **More calculations** opens a searchable library of all 50 functions, grouped by category, with plain descriptions, named arguments, and examples.

**Try your calculation** previews the result immediately. Use editable sample values or select an existing record; preview edits never write to that record. Referenced calculated fields are recomputed using the current field drafts, rather than their stored response values. Formula syntax/reference status is shown separately from runtime errors such as division by zero.

Choose **Edit formula** for nested/custom expressions. Typing `[` shows matching fields. **Browse functions** can insert a function at the cursor or wrap selected text while preserving the surrounding expression. Replacing a calculation with a new guided function is a separate, explicit action. Saved formulas that the builder can represent open with their named inputs; other expressions remain in the advanced editor and are retained unchanged until edited.

Save fields to apply the calculation to the collection. Calculated fields are read-only and update automatically when their inputs change; they do not have a Required checkbox. Percentages return numbers such as `25` for 25%. Lists come from array/JSON fields rather than literal arrays in formulas.

```json
{"fields":[
  {"name":"price","type":"number"},
  {"name":"quantity","type":"number"},
  {"name":"total","type":"calculated","formula":"=ROUND([price]*[quantity],2)"}
]}
```

Use `[field_name]`, finite numbers, `TRUE`/`FALSE`, double-quoted text, parentheses, `+ - * / ^ &` and comparisons `= <> < > <= >=`. Function names are case-insensitive. Quotes inside a string are doubled: `"say ""hello"""`. Calculated fields can reference other calculated fields in any blueprint order. Missing fields, circular dependencies, unsupported functions and incorrect argument counts are rejected when saving the blueprint.

Calculated fields are read-only and cannot be required. The server strips supplied calculated values before storage and derives them from the current blueprint on every read/write response, including public endpoints. Formula changes therefore apply to existing records immediately without a backfill. No database migration is needed for existing blueprints. Existing numeric fields can become calculated fields; returning to a writable field does not guarantee that previous stored calculated values remain available.

Evaluation failures produce `null` and a `calculation_errors` object keyed by field name in the document response. They appear in the record table/editor. `IF` evaluates only its selected branch; `IFERROR` evaluates its fallback when its first argument fails. Ordinary records can save with calculation errors, allowing incomplete work; bulk import requires every calculated field to succeed.

## Function catalog

The catalog is a practical selection of 50 commonly used Excel functions, not an official frequency ranking. This engine adapts them to named fields and JSON arrays within one record; it does not implement Excel workbooks or the full Excel coercion/formatting system.

| Category | Functions |
| --- | --- |
| Aggregation | SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, COUNTBLANK, PRODUCT, MEDIAN |
| Math | ABS, ROUND, ROUNDUP, ROUNDDOWN, INT, MOD, POWER, SQRT, CEILING, FLOOR |
| Logic | IF, IFERROR, AND, OR, NOT |
| Text | CONCAT, CONCATENATE, TEXTJOIN, LEFT, RIGHT, MID, LEN, LOWER, UPPER, PROPER, TRIM, SUBSTITUTE, REPLACE, FIND, SEARCH, EXACT, VALUE |
| Dates | DATE, YEAR, MONTH, DAY |
| Conditional aggregation and lookup | SUMIF, COUNTIF, AVERAGEIF, INDEX, MATCH |

Examples: `SUM([amounts])`, `COUNTIF([tags], "a*")`, `SUMIF([amounts], ">100")`, `INDEX([amounts], 1)`, `MATCH("target", [tags], 0)`, `TEXTJOIN(", ", TRUE, [tags])`.

Aggregation flattens arrays and includes numeric JSON values, ignoring null/text/boolean values. COUNTA counts empty strings; COUNTBLANK counts null and empty strings. Arithmetic treats missing/null values as zero and accepts plain numeric text. VALUE parses plain numbers, including scientific notation; currency, localized separators and date text conversion are outside its scope. Text positions/counts use Unicode characters and are one-based for MID/FIND/SEARCH/REPLACE. TRIM collapses ordinary spaces. Conditional aggregation accepts comparison criteria and text wildcards `*`, `?`, `~`; corresponding arrays must have equal lengths. Wildcard patterns are capped at 256 characters. SEARCH and exact MATCH also support wildcards. Approximate MATCH assumes a sorted array (ascending for 1, descending for -1); use mode 0 for exact lookup.

DATE returns an Excel serial date using the 1900 date system; YEAR/MONTH/DAY accept that serial or an ISO `YYYY-MM-DD` string. Excel's fictitious 1900-02-29 (serial 60) is rejected. Cell coordinates/ranges (`A1:B10`), cross-record/collection aggregation, formatted TEXT, volatile functions and imported workbook formula execution are outside this version. Formula size is capped at 4 KB, expression depth at 64, operations at 512, dependency chains at 64, function inputs and text results at 1 MB. Wildcard evaluations have a one-million-step limit. Numeric calculations use floating point, as in ordinary spreadsheet arithmetic.

## Import workflow

1. Save the collection's fields.
2. Open Import data and select a UTF-8 CSV or XLSX file. Its first populated row must contain nonempty, unique headers.
3. Read the file, choose an XLSX worksheet if necessary, and map each column to a writable field or Skip column. Matching field names are mapped automatically. Targets must be unique.
4. Validate and review the first ten converted rows, including calculated values. Error messages identify source rows and fields when available. The UI shows the first 100 errors.
5. Import the validated batch. The server reparses and revalidates it under the current schema in the write transaction. Any error results in zero written records.

Imports append new documents, not updates/upserts. Empty cells leave optional fields unset and fail required fields. Boolean cells accept true/false, yes/no or 1/0. Numeric cells must be finite JSON numbers. Array, JSON and Portable Text cells contain JSON. Asset and reference cells contain existing IDs and are checked against their collection. Formulas in XLSX use saved cached values: recalculate/save the workbook in Excel before importing. The reader does not execute macros or formulas.

Limits: 10 MB uploaded file, 1,000 data rows, 200 columns and 50 MB expanded XLSX archive. For allocation safety, all workbook cells must lie within rows 1–1,001 and columns 1–200, including unused worksheets. Export a smaller workbook if needed. Save old `.xls` files as `.xlsx` first.

API: `POST /api/v1/collections/{id}/import` with multipart fields `file`, optional `sheet`, optional `mapping` (JSON array of field names or null, in header order), and `commit` (`false` by default). The result includes headers, worksheets, mapping, row_count, up to ten preview bodies, errors and imported count. A valid preview and a validation-failing commit both return HTTP 200 with structured results; malformed files/options return 400. Authentication and collection-scoped API-key rules match document writes. Repeating a successful commit intentionally appends another batch; there is no idempotency key yet.

## Import updates and export

The collection Import data tab offers append and create/update modes. For updates, choose a string or number field that uniquely identifies existing records. The matching column must be mapped. Blank cells keep existing values; unmapped fields are preserved. Duplicate file keys or existing matching values block the complete batch. Equivalent decimal numeric keys (for example `1` and `1.0`) match without collapsing distinct large integers. String keys match exactly and case-sensitively.

The import multipart API accepts `mode=append` (default) or `mode=upsert` with `match_field=sku`, in addition to file, mapping, sheet and commit. Preview and commit report `created`, `updated`, `unchanged`, and `preview_actions` alongside the original fields. `imported` counts actual writes; unchanged rows retain their timestamps and do not add revisions. Commit revalidates current data within one transaction, so preview counts can change if records change before commit.

For example:

```bash
curl -X POST "http://127.0.0.1:8080/api/v1/collections/COLLECTION_ID/import" \
  -H "Authorization: Bearer $VULSE_API_KEY" \
  -F 'file=@inventory.csv' -F 'mode=upsert' -F 'match_field=sku' -F 'commit=false'
```

The Records tab exports the current filtered view across all pages, or the selected rows, with visible fields. POST `/api/v1/collections/COLLECTION_ID/export` with `format`, `columns` and `query`, or `ids` for a selection. JSON downloads contain record envelopes with only requested body fields; CSV uses API field headers and JSON for structured cells. CSV prefixes spreadsheet formula-like text with an apostrophe; JSON preserves exact values. Exports reject results over 10,000 records / 25 MB and selections over 100 unique IDs.
