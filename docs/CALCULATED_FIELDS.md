# Calculated fields and imports

Add a field with type **Calculated** in the Fields view. The guided builder starts with **Multiply values**. Choose a calculation and fill its named inputs using typed field pickers or fixed values. Common tasks include adding values, percentages, combining text, rounding, and if/otherwise comparisons. When other collections exist, **From another collection** builds formulas that read them (see [Reading other collections](#reading-other-collections)). **More calculations** opens a searchable library of all 56 functions, grouped by category, with plain descriptions, named arguments, and examples.

**Try your calculation** previews the result immediately. Use editable sample values or select an existing record; preview edits never write to that record. Referenced calculated fields are recomputed using the current field drafts, rather than their stored response values. Formula syntax/reference status is shown separately from runtime errors such as division by zero.

Choose **Edit formula** for nested/custom expressions. Typing `[` shows matching fields, including other collections' fields as `[collection.field]`. **Browse functions** can insert a function at the cursor or wrap selected text while preserving the surrounding expression. Replacing a calculation with a new guided function is a separate, explicit action. Saved formulas that the builder can represent open with their named inputs; other expressions remain in the advanced editor and are retained unchanged until edited.

Save fields to apply the calculation to the collection. Calculated fields are read-only and update automatically when their inputs change; they do not have a Required checkbox. Percentages return numbers such as `25` for 25%. Lists come from array/JSON fields or from other collections' columns rather than literal arrays in formulas.

```json
{"fields":[
  {"name":"price","type":"number"},
  {"name":"quantity","type":"number"},
  {"name":"total","type":"calculated","formula":"=ROUND([price]*[quantity],2)"}
]}
```

## Formula syntax

Use `[field_name]` for a field of the same record, `[collection.field]` for a field of another collection, finite numbers, `TRUE`/`FALSE`, double-quoted text, parentheses, `+ - * / ^ &` and comparisons `= <> < > <= >=`. Function names are case-insensitive. Quotes inside a string are doubled: `"say ""hello"""`. Arguments can be separated with `,` or `;`, so formulas copied from spreadsheets in decimal-comma locales work as is. Numbers in a formula always use a point as the decimal mark (`1.5`); with `;` as a separator, `1,5` would read as two arguments.

Calculated fields can reference other calculated fields in any blueprint order. Missing fields, unknown collections, circular dependencies, unsupported functions and incorrect argument counts are rejected when saving the blueprint.

Calculated fields are read-only and cannot be required. The server strips supplied calculated values before storage and derives them from the current blueprint on every read/write response, including public endpoints. Formula changes therefore apply to existing records immediately without a backfill. No database migration is needed for existing blueprints. Existing numeric fields can become calculated fields; returning to a writable field does not guarantee that previous stored calculated values remain available.

Evaluation failures produce `null` and a `calculation_errors` object keyed by field name in the document response. They appear in the record table/editor. `IF` evaluates only its selected branch; `IFERROR` evaluates its fallback when its first argument fails. Ordinary records can save with calculation errors, allowing incomplete work; bulk import requires every calculated field to succeed.

## Function catalog

The catalog is a practical selection of 56 commonly used Excel functions, not an official frequency ranking. This engine adapts them to named fields, JSON arrays and other collections' columns; it does not implement Excel workbooks or the full Excel coercion/formatting system.

| Category | Functions |
| --- | --- |
| Aggregation | SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, COUNTBLANK, PRODUCT, MEDIAN |
| Math | ABS, ROUND, ROUNDUP, ROUNDDOWN, INT, MOD, POWER, SQRT, CEILING, FLOOR |
| Logic | IF, IFERROR, AND, OR, NOT |
| Text | CONCAT, CONCATENATE, TEXTJOIN, LEFT, RIGHT, MID, LEN, LOWER, UPPER, PROPER, TRIM, SUBSTITUTE, REPLACE, FIND, SEARCH, EXACT, VALUE |
| Dates | DATE, YEAR, MONTH, DAY |
| Conditional aggregation | SUMIF, COUNTIF, AVERAGEIF, SUMIFS, COUNTIFS, AVERAGEIFS, MINIFS, MAXIFS |
| Lookup | XLOOKUP, INDEX, MATCH |

Signatures of the less common ones:

| Function | Usage |
| --- | --- |
| SUMIF | `SUMIF(array, criteria, sum_array?)` |
| AVERAGEIF | `AVERAGEIF(array, criteria, average_array?)` |
| SUMIFS | `SUMIFS(sum_array, criteria_array, criteria, …)` |
| COUNTIFS | `COUNTIFS(criteria_array, criteria, …)` |
| AVERAGEIFS | `AVERAGEIFS(average_array, criteria_array, criteria, …)` |
| MINIFS / MAXIFS | `MINIFS(min_array, criteria_array, criteria, …)` |
| XLOOKUP | `XLOOKUP(value, lookup_array, return_array, if_not_found?)` |
| INDEX | `INDEX(array, position)` |
| MATCH | `MATCH(value, array, mode?)` |

Aggregation flattens arrays and includes numeric JSON values, ignoring null/text/boolean values. COUNTA counts empty strings; COUNTBLANK counts null and empty strings. Arithmetic treats missing/null values as zero and accepts plain numeric text. VALUE parses plain numbers, including scientific notation; currency, localized separators and date text conversion are outside its scope. Text positions/counts use Unicode characters and are one-based for MID/FIND/SEARCH/REPLACE. TRIM collapses ordinary spaces. SEARCH and exact MATCH also support wildcards. Approximate MATCH assumes a sorted array (ascending for 1, descending for -1); use mode 0 for exact lookup.

**Criteria** in SUMIF, COUNTIF, AVERAGEIF and the `IFS` functions are a value or text starting with a comparison: `2024`, `"Oslo"`, `">100"`, `"<>closed"`, `">="&[from_year]`. Numbers and numeric text match by value, so `2024` matches `"2024"`. Other text matches case-insensitively. Text criteria accept the wildcards `*`, `?` and `~` (escape); patterns are capped at 256 characters. Corresponding arrays must have equal lengths.

**Results when nothing matches:** SUMIFS and COUNTIFS return `0`, MINIFS and MAXIFS return `0`, AVERAGEIFS fails with `#DIV/0!`, and XLOOKUP fails with `#N/A` unless it has an `if_not_found` argument. Only numeric values are summed, averaged or compared by the `IFS` functions; text in the target list is skipped, even numeric text such as `"12"`.

DATE returns an Excel serial date using the 1900 date system; YEAR/MONTH/DAY accept that serial or an ISO `YYYY-MM-DD` string, so they work directly on date fields: `=YEAR([due])`. Excel's fictitious 1900-02-29 (serial 60) is rejected.

## Examples within one record

| Goal | Formula |
| --- | --- |
| Line total, rounded | `=ROUND([price]*[quantity];2)` |
| Price with VAT | `=[price]*(1+[vat_rate]/100)` |
| Share in percent | `=IFERROR([part]/[whole]*100;"")` |
| Full name | `=TRIM([first_name]&" "&[last_name])` |
| Status label | `=IF([balance]<0;"Underfunded";"Funded")` |
| Several conditions | `=IF(AND([is_active];[score]>=50);"Yes";"No")` |
| Year of a date field | `=YEAR([signed_on])` |
| Years since a year | `=2026-[founded_year]` |
| Sum of a number-list field | `=SUM([monthly_amounts])` |
| Amounts over a threshold in a list | `=SUMIF([monthly_amounts];">1000")` |
| Tags as text | `=TEXTJOIN(", ";TRUE;[tags])` |
| Position of a tag | `=IFERROR(MATCH("urgent";[tags];0);0)` |

## Reading other collections

`[collection.field]` is that field's values across all records of the collection, like a column on another spreadsheet sheet, in the order the records were created. Use it in the conditional aggregation functions and XLOOKUP to pick out matching records, or in plain aggregation such as `SUM` and `COUNT` for the whole column. A column is a list, so it cannot be used in arithmetic directly: `[settings.rate]*2` fails, while `INDEX([settings.rate];1)*2` works.

Rules:

- Every list in one function must come from the same collection, so a function cannot combine columns of two collections, or a column with a list field of the current record. Criteria values can come from anywhere, usually from the current record.
- A formula can read calculated fields of another collection, but not ones that read other collections themselves, so lookups never chain. Saving fields checks this, and checks that each referenced collection and field exists.
- A formula can read its own collection, for example to compare a record with the collection's total.
- References use the collection's name. Renaming a collection or a field that formulas read rewrites those formulas. Removing such a field or deleting the collection is refused until the formulas change.
- Record reads, including the API, public reads and exports, always calculate from the other collection's current data. Filters, sorting and summaries use the collection's record table, which is rewritten in the same transaction whenever a collection its formulas read changes, so they match too. On large collections that rewrite makes writes to the collections they read slower. The record editor shows the saved value for these fields instead of a live preview.
- Formula results are visible to anyone who can read the record, including public readers, even when they cannot read the other collection. Don't put a formula reading private data in a public collection unless its result may be public.
- Vulse SQL can read these fields but cannot call SUMIFS, COUNTIFS, AVERAGEIFS, MINIFS, MAXIFS or XLOOKUP; use `WHERE` and `GROUP BY` there instead.

The guided **From another collection** calculation builds these formulas without typing: choose the collection, what to calculate (sum, average, smallest, largest, count of matching records, or the value from the first matching record) and the conditions that pick the matching records. Each condition compares one of their fields with a field of this record (optionally plus or minus a number, such as the year minus 2) or with a fixed value. It shows the calculation as a sentence and the resulting formula, and formulas of this shape reopen in the guided view.

### Examples across collections

These examples assume collections named `orders` (`customer_id`, `amount`, `status`, `year`), `products` (`sku`, `name`, `price`) and `settings` (one record with `name`, `growth`, `inflation`). Each formula sits in a calculated field of another collection, such as `customers` with an `id` field.

| Goal | Formula |
| --- | --- |
| Total ordered by this customer | `=SUMIFS([orders.amount];[orders.customer_id];[id])` |
| Number of open orders | `=COUNTIFS([orders.customer_id];[id];[orders.status];"open")` |
| Average order this year | `=IFERROR(AVERAGEIFS([orders.amount];[orders.customer_id];[id];[orders.year];2026);"")` |
| Largest single order | `=MAXIFS([orders.amount];[orders.customer_id];[id])` |
| Orders from a year onward | `=SUMIFS([orders.amount];[orders.customer_id];[id];[orders.year];">="&[from_year])` |
| Product name by SKU | `=XLOOKUP([sku];[products.sku];[products.name];"Unknown")` |
| Line value from a price list | `=[quantity]*XLOOKUP([sku];[products.sku];[products.price];0)` |
| Collection-wide total | `=SUM([orders.amount])` |
| Share of all orders, in percent (in `orders` itself) | `=IFERROR([amount]/SUM([orders.amount])*100;"")` |
| A setting from a one-record collection | `=INDEX([settings.growth];1)` |
| A setting picked by name | `=XLOOKUP("default";[settings.name];[settings.growth])` |

**Values from several years.** Keep each year's data as records in one collection with a year field, rather than one collection per year, and select the year in the criteria. For example, with yearly costs in `costs` (`org_nr`, `year`, `amount`) and growth rates in `settings`, a summary collection with one record per organization can project the last three years to today's level:

```
=IFERROR(
   SUMIFS([costs.amount];[costs.org_nr];[org_nr];[costs.year];2024)
     *(1+INDEX([settings.growth];1))*(1+INDEX([settings.inflation];1))
 + SUMIFS([costs.amount];[costs.org_nr];[org_nr];[costs.year];2025)
     *(1+INDEX([settings.growth];1))
 + SUMIFS([costs.amount];[costs.org_nr];[org_nr];[costs.year];2026);
 "")
```

Years can also be relative to a field of the record, so the formula keeps working when a new year is added. With a `year` field on the summary record, a weighted average of the two previous years is:

```
=IFERROR(SUMIFS([basil.arsverk];[basil.org_nr];[org_nr];[basil.ar];[year]-2);"")*7/12
 + IFERROR(SUMIFS([basil.arsverk];[basil.org_nr];[org_nr];[basil.ar];[year]-1);"")*5/12
```

Columns are cached until the other collection's records or fields change. Matching on plain values (such as `[orders.customer_id];[id]`) uses an index, so lookups stay fast on large collections.

## Limits

Cell coordinates/ranges (`A1:B10`), formatted TEXT, volatile functions and imported workbook formula execution are outside this version. Formula size is capped at 4 KB, expression depth at 64, operations at 512, dependency chains at 64, function inputs and text results at 1 MB. Wildcard evaluations have a one-million-step limit. Numeric calculations use floating point, as in ordinary spreadsheet arithmetic.

## Import workflow

1. Save the collection's fields.
2. Open Import data and select a CSV or XLSX file. By default its first row holds nonempty, unique column headers. Untick **First row contains column headers** for a file without one; its columns are then named `Column 1`, `Column 2` and so on.
3. Read the file, choose an XLSX worksheet if necessary, and map each column to a writable field or Skip column. Matching field names are mapped automatically. Targets must be unique.
4. Validate and review the first ten converted rows, including calculated values. Error messages identify source rows and fields when available. The server reports up to 500 problems and the UI shows the first 100.
5. Import the validated batch. The server reparses and revalidates it under the current schema in the write transaction. Any error results in zero written records.

CSV files must be UTF-8 (a byte order mark is accepted). The separator is detected from the header line: `,`, `;` or tab, so semicolon-separated exports from decimal-comma locales work. Empty cells leave optional fields unset and fail required fields. Boolean cells accept true/false, yes/no or 1/0. Numeric cells must be finite numbers; a comma is read as the decimal mark (`33,47`) when the cell has no point. A value like `1,234` (one to three digits, a comma, then exactly three digits) is refused as ambiguous, since it could be 1234 or 1.234; write it as `1234` or `1.234`. `0,125` and `1234,567` are not ambiguous and import as decimals. Date cells must be `YYYY-MM-DD`; date-formatted XLSX cells are converted when they hold a whole day, and a time of day is refused rather than dropped. Array, JSON and Portable Text cells contain JSON. Asset and reference cells contain existing IDs and are checked against their collection. Formulas in XLSX use saved cached values: recalculate/save the workbook in Excel before importing. The reader does not execute macros or formulas, and cells holding a spreadsheet error such as `#DIV/0!` are refused.

Limits: 50 MB uploaded file, 50,000 data rows and 200 columns per file or worksheet, and 200 MB expanded XLSX archive or worksheet. Save old `.xls` files as `.xlsx` first.

API: `POST /api/v1/collections/{id}/import` with multipart fields `file`, optional `sheet`, optional `mapping` (JSON array of field names or null, in header order), optional `has_headers` (`true` by default), and `commit` (`false` by default). The result includes headers, worksheets, mapping, row_count, up to ten preview bodies, errors and imported count. A valid preview and a validation-failing commit both return HTTP 200 with structured results; malformed files/options return 400. Authentication and collection-scoped API-key rules match document writes. In the default append mode, repeating a successful commit intentionally appends another batch; there is no idempotency key yet.

## Import updates and export

The collection Import data tab offers append and create/update modes. For updates, choose a string or number field that uniquely identifies existing records. The matching column must be mapped. Blank cells keep existing values; unmapped fields are preserved. Duplicate file keys or existing matching values block the complete batch. Equivalent decimal numeric keys (for example `1` and `1.0`) match without collapsing distinct large integers. String keys match exactly and case-sensitively.

The import multipart API accepts `mode=append` (default) or `mode=upsert` with `match_field=sku`, in addition to file, mapping, sheet, has_headers and commit. Preview and commit report `created`, `updated`, `unchanged`, and `preview_actions` alongside the original fields. `imported` counts actual writes; unchanged rows retain their timestamps and do not add revisions. Commit revalidates current data within one transaction, so preview counts can change if records change before commit.

For example:

```bash
curl -X POST "http://127.0.0.1:8080/api/v1/collections/COLLECTION_ID/import" \
  -H "Authorization: Bearer $VULSE_API_KEY" \
  -F 'file=@inventory.csv' -F 'mode=upsert' -F 'match_field=sku' -F 'commit=false'
```

The Records tab exports the current filtered view across all pages, or the selected rows, with visible fields. POST `/api/v1/collections/COLLECTION_ID/export` with `format`, `columns` and `query`, or `ids` for a selection. JSON downloads contain record envelopes with only requested body fields; CSV uses API field headers and JSON for structured cells. CSV prefixes spreadsheet formula-like text with an apostrophe; JSON preserves exact values. Exports reject results over 10,000 records / 25 MB and selections over 100 unique IDs.
