# Vulse

Vulse stores JSON documents in named collections. Each collection has a blueprint that describes its fields, and each document is checked against that blueprint. All collections live in one shared workspace. People get access through an admin invite and have a role: viewer, editor or admin. A collection is either public or private. Public documents can be read without an account. Private ones need a signed-in session or an API key.

Longer text fields use a small subset of [Portable Text](https://portabletext.org/). Images, video, audio, and PDFs can be attached to a collection and referenced from documents.

## Start

You need Rust and Node.js 22 or later. The admin UI is a WebAssembly app, so the first run also needs the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev/).

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
```

Build the admin, then start the server from the repository root:

```bash
npm --prefix crates/web ci
trunk build --config crates/web/Trunk.toml
cargo run -p vulse-server
```

The server listens on `127.0.0.1:8080`. On first start with an empty database it logs a one-time setup token. Open `/setup` in a browser and use that token to create the first admin (see [Accounts and access](#accounts-and-access)). The admin is served from `crates/web/dist`. If that directory is missing, the API still runs and the site routes are absent.

While changing the UI, run Trunk's dev server in another terminal. It serves the admin on `127.0.0.1:8081` and proxies `/api` to the Axum process:

```bash
trunk serve --config crates/web/Trunk.toml
```

With no Turso URL set, data is stored in `./vulse.db` and uploaded files in `./storage`. Both are gitignored.

| Variable | Default | Purpose |
| --- | --- | --- |
| `BIND` | `127.0.0.1:8080` | Listen address |
| `VULSE_DB_PATH` | `./vulse.db` | Local database file |
| `ASSET_DIR` | `./storage` | Upload directory |
| `VULSE_WEB_DIST` | `crates/web/dist` | Built admin files |
| `TURSO_DATABASE_URL` | unset | Remote Turso database |
| `TURSO_AUTH_TOKEN` | unset | Token for that database |
| `VULSE_PUBLIC_URL` | unset | External address, such as `https://cms.example.com`. An `https://` value turns on secure cookies. Also accepted as the origin for signed-in writes |
| `VULSE_COOKIE_SECURE` | from `VULSE_PUBLIC_URL` | `true` adds `Secure` and the `__Host-` prefix to the session cookie. Set it when TLS ends at a proxy and `VULSE_PUBLIC_URL` is unset |
| `VULSE_SETUP_TOKEN` | random | Fixed first-run setup token, at least 16 characters. Only used while no accounts exist |
| `VULSE_TRUST_PROXY` | `false` | Take the client address from the last `X-Forwarded-For` entry. Enable only behind a proxy that sets it |

Set both Turso variables, or neither. The remote database should be created with `turso db create --tursodb`, because the server uses the current Turso Rust engine.

In production, serve Vulse over HTTPS and set `VULSE_PUBLIC_URL`.

## Accounts and access

There is no public sign-up. All accounts belong to one shared workspace, and an admin decides who joins.

### First admin

When the database has no accounts, the server logs a setup token at startup:

```text
WARN No accounts exist yet. Open http://127.0.0.1:8080/setup and enter this setup token to create the first admin: …
```

Open `/setup` (the login page redirects there on a fresh install), enter the token, and choose an email, handle and password. Set `VULSE_SETUP_TOKEN` to use a fixed token instead of a random one. After the first account exists, setup is refused. With the API:

```bash
curl -c cookies.txt -H 'content-type: application/json' \
  -d '{"token":"…","email":"ada@example.com","handle":"ada","password":"correct-horse"}' \
  http://127.0.0.1:8080/api/v1/auth/setup
```

The handle is 2–32 characters of lowercase letters, digits, or hyphens and is shown to other members. The password is 8–128 characters. Email and handle must be unique.

### Roles

| Role | Can |
| --- | --- |
| Viewer | Read every collection, record, asset, saved view, history entry and backlink; query and export |
| Editor | Everything a viewer can, plus create, edit, delete, bulk-edit, undo and restore records; upload and delete assets; import; manage saved views |
| Admin | Everything an editor can, plus create, change and delete collections, and manage people |

The server enforces roles on every request. The admin hides controls your role cannot use. The workspace always keeps at least one active admin: demoting or disabling the last one is refused.

### Inviting people

Admins manage members on the **People** page or with the API (an admin session is required; API keys cannot manage people):

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/v1/users` | List members, roles and status |
| `PATCH` | `/api/v1/users/{id}` | `{"role":"editor"}` and/or `{"disabled":true}` |
| `POST` | `/api/v1/users/{id}/password-reset` | Issue a password reset link |
| `GET` | `/api/v1/invites` | List pending invite and reset links |
| `POST` | `/api/v1/invites` | `{"email":"grace@example.com","role":"editor"}` |
| `DELETE` | `/api/v1/invites/{id}` | Revoke a pending link |

Creating an invite or a reset returns a single-use link such as `/accept#token=…`. Vulse does not send email. The admin copies the link and sends it privately. The token is in the URL fragment, so it is not sent to the server in requests or in `Referer` headers, and the accept page removes it from the address bar. Invites expire after 72 hours and reset links after 24 hours. A new link for the same address replaces the previous one. The recipient opens the link, chooses a handle (for invites) and a password, and is signed in.

Disabling a member ends their sessions and stops their API keys at once. Enabling them restores the keys. Accounts are disabled, not deleted. Collections and uploads keep a reference to the account that created them.

### Your account

The **Account** page (click your handle) changes your password and signs out everywhere. Changing the password requires the current one and signs out your other sessions. A reset link also signs out every session of that account. The API equivalents are `POST /api/v1/auth/password` with `{"current_password":"…","new_password":"…"}` and `POST /api/v1/auth/logout-everywhere`.

### Sessions

Login, setup and accepting a link set an HttpOnly, `SameSite=Lax` session cookie that lasts 14 days. It is named `vulse_session`, or `__Host-vulse_session` with `Secure` when secure cookies are on. The database stores only a SHA-256 hash of each session token, so a copy of the database cannot be used to sign in. Expired sessions and links are deleted hourly.

Writes signed in with the cookie are refused when the browser reports a cross-site request: `Sec-Fetch-Site` of `cross-site` or `same-site`, or, in older browsers, an `Origin` that matches neither the `Host` header nor `VULSE_PUBLIC_URL`. Requests with an `Authorization` header are not affected. Admin pages are served with `X-Frame-Options: DENY`.

### Sign-in limits

Failed sign-ins are counted for 15 minutes: after 5 for one account, or 30 from one address (which also covers setup tokens and link lookups), further attempts get `429` with a `Retry-After` header, even with the right password. A successful sign-in clears the account's count. Unknown addresses go through the same password check as real ones, so response times do not reveal which accounts exist. Password hashing runs on four shared workers. A sign-in waits up to 10 seconds for one before answering `429`.

These counters are kept in memory for one server process. If you run several instances, also rate-limit `/api/v1/auth/` at your proxy. Behind a proxy, set `VULSE_TRUST_PROXY=true` so limits apply per client rather than to the proxy's address.

### API keys

API keys are created while signed in, from the API keys page or `POST /api/v1/api-keys`. The raw secret (`vulse_…`) is returned once and stored as a SHA-256 hash. Send it as `Authorization: Bearer vulse_…`.

A key has a role of its own, which defaults to yours and cannot be higher. Use `{"name":"site","role":"viewer"}` for a read-only key. A key never has more access than its owner: if the owner's role is lowered, the key's effective role is lowered too. A disabled owner's keys stop working. A key can cover the whole workspace or a single collection with `collection_id`. Collection-scoped keys cannot reach other collections or change schemas, and no key can manage people or other keys.

### Public reads

Public reads do not use the cookie or the key:

```text
GET /api/v1/public/{collection}
GET /api/v1/public/{collection}/{document_id}
GET /api/v1/public/{collection}/assets/{asset_id}
```

A missing collection and a private collection both respond with 404. Collection names are unique in the workspace.

### Upgrading an existing database

Migration 6 converts a database from the earlier one-workspace-per-account model:

- Every existing account becomes an admin, and every existing API key gets the admin role, so nobody loses access.
- Collection names become unique across the workspace. If two accounts used the same name, the oldest collection keeps it and later ones get the creator's handle appended (`posts` → `posts-grace`). Reference fields in that creator's other collections are updated to match. Each rename is logged as a warning.
- Existing sessions are dropped, because they were stored in plaintext. Everyone signs in once more.
- Public URLs no longer include a handle: `/api/v1/public/{handle}/{collection}` becomes `/api/v1/public/{collection}`.

## Architecture

```text
crates/model    shared types and validation, compiled for the server and for wasm
crates/server   Axum API, Turso access, sessions, and the file store
crates/web      Yew admin, built with Trunk
```

The admin and other clients talk to Axum. Axum writes rows to Turso and file bytes to disk. Document and asset ids in the database are text UUIDs. The original upload name is metadata only. The file on disk is named `{owner id}/{asset id}`.

A blueprint is an ordered field list: string, number, boolean, date, free-form JSON, Portable Text, asset, reference, or an array of string, number, asset, or reference. A date is a calendar day stored and returned as a `"YYYY-MM-DD"` string, with no time or time zone. The admin edits it with the browser's date picker. Record filters can compare dates (before, after, on or before, on or after), and sorting a date field orders records by day. Formulas read dates directly: `=YEAR([due])`. CSV imports accept `YYYY-MM-DD`. Date-formatted XLSX cells are converted when they hold a whole day; cells with a time of day are refused for date fields rather than truncated. The server rejects unknown document keys and checks that references and assets exist. Saving a blueprint rechecks every document already in the collection. If any would become invalid, the change is refused with the first validation error and the ids of the affected documents. Removing a field that holds values is refused unless the request sets `"remove_field_data": true`; the values are then deleted from every record in the same transaction, and each record's previous version stays in its history (shown as "fields removed"). Changing a field's type converts its stored values after you confirm: text such as `"12"` or `" 3.5 "` becomes a number, `yes`/`no`, `true`/`false` and `1`/`0` become true/false, `YYYY-MM-DD` text becomes a date, and numbers, true/false values, dates and rich text become text. Blank text becomes empty. Decimal commas (`1,5`) and other date formats are not converted, because they are ambiguous. If any value cannot be converted, nothing changes and the error names the records and values to fix. Converted records get a "fields converted" history entry. Over the API, send `"convert_field_values": true` with the schema update; without it, a type change that existing values don't fit is refused with a hint. Renaming a field keeps its data: in the field editor, changing a saved field's name moves its values, the formula references to it, the title field setting and saved view filters, sorting, grouping and columns to the new name when you save. Over the API, mark the field with `"renamed_from": "old_name"` in the blueprint sent with `PATCH /api/v1/collections/{id}`; without it, a changed name is treated as removing the old field and adding a new one. Records changed by a rename get a "fields renamed" history entry. Older history versions keep the old field name, so restoring one after a rename can fail validation until the field is renamed back. In the admin, removing a saved field and other destructive actions ask for confirmation in a modal first.

Collections belong to the workspace, not to the account that created it. Passwords are hashed with Argon2id. Session tokens, API key secrets and invite/reset tokens are stored as SHA-256 hashes. Role checks happen in Axum extractors (`AuthUser`, `Editor`, `SchemaAdmin`, `SessionOnly`, `AdminSession` in `crates/server/src/auth.rs`); sign-in throttling and the cross-site write check are in `crates/server/src/security.rs`.

## Collection workspace

The admin opens collections on a searchable record table, with separate Fields, Import data, API, Trash and Settings views. The record table updates live: the server announces changes to the collection, and to collections its formulas read or its references point to, over Server-Sent Events (`GET /api/v1/collections/{id}/events`, a `change` event per change), and the table reloads quietly, without clearing or flashing. Announcements are per server process, so the table also checks for changes every minute while the tab is visible. A calculated field uses a formula such as `=ROUND([price] * [quantity], 2)` and supports 56 Excel-style functions. Results are derived on the server from current data and formulas, including public API reads. The formula box completes as you type: start a function name or type `[` for fields, including other collections' fields as `[collection.field]`, then choose with the arrow keys and press Enter or Tab. While the caret is inside a function, the box shows its usage, what the current argument expects, a description and an example. It also reports syntax errors and unknown fields, and **Browse functions** opens a searchable library of every function. Summary formulas use the same box, suggesting the summary's other values.

Formulas can read other collections. `[collection.field]` is that field's values across all records of the collection, like a column on another spreadsheet sheet, in the order the records were created. Use it with `SUMIFS`, `COUNTIFS`, `AVERAGEIFS`, `MINIFS`, `MAXIFS` and `XLOOKUP`, which work like their Excel namesakes. Criteria match numbers and numeric text by value and other text case-insensitively; `">10"` and wildcards work as in `SUMIF`. Arguments can be separated with `,` or `;`, so formulas pasted from spreadsheets in many locales work as is. For example, a pension record can weight the full-time equivalents reported for its organization over two years:

```
=IFERROR(SUMIFS([basil.arsverk]; [basil.org_nr]; [org_nr]; [basil.ar]; [ar]-2); "")*7/12
 + IFERROR(SUMIFS([basil.arsverk]; [basil.org_nr]; [org_nr]; [basil.ar]; [ar]-1); "")*5/12
```

In the field editor, the guided **From another collection** calculation builds these formulas without typing: choose the collection, what to calculate (sum, average, smallest, largest, count of matching records, or the value from the first matching record) and the conditions that pick the matching records, each comparing one of their fields with a field of this record (optionally plus or minus a number, such as the year minus 2) or a fixed value. It shows the calculation as a sentence and the resulting formula, and formulas of this shape reopen in the guided view. Every list in one function must come from the same collection. A formula can read calculated fields of another collection, but not ones that read other collections themselves, so lookups never chain. Saving fields checks that each referenced collection and field exists. Renaming a collection or a field it reads rewrites the formulas, and removing such a field or deleting the collection is refused until the formulas change. Record reads, including the API and exports, always calculate from the other collection's current data. Filters, sorting and summaries use the value stored when the record was last saved, or when its collection's fields were last saved, so they can lag behind changes in the other collection. The record editor shows the saved value for these fields instead of a live preview. Formula results are visible to anyone who can read the record, including public readers, even when they cannot read the other collection.

Fields can have a display label, description and example while their API names remain stable. In Fields, choose a title field for readable record links and reference choices. Search, the view picker and the record count sit in one toolbar row above the table. Its **Filter**, **Sort**, **Columns**, **Save view…** and **Export** buttons each open one panel below the toolbar, so the table stays in view. Active filters appear as chips that can be removed or clicked to edit. Saved views preserve typed filters, sort order, grouping and visible columns. Search and pagination run on indexed server values; each refresh fetches the visible page.

Click a text, number or boolean cell to edit it. Enter saves and Escape cancels. Select rows on the current page to update one field, duplicate, delete or export them. **Undo last action** reverses a batch within 24 hours if its records have not changed. Referenced targets cannot be deleted until incoming links are removed; a batch may delete records that only reference one another.

Unsaved record and collection drafts are stored under your account on this device. On returning, choose Restore or Discard. Older record drafts can apply only their changed fields to the latest record; when both edits change the same field, the explicit merge keeps the draft value. Review older schema drafts against the current fields before choosing **Use latest collection version**. Incomplete number/JSON inputs are retained. Storage failures are shown visibly. Saved record history includes field differences and restore controls, and **Trash** recovers deleted record IDs. Existing data receives a baseline version during migration; history cannot reconstruct earlier edits. Restores validate against the current schema, references and files.

Reference fields use a searchable picker: a new record starts with nothing selected, typing searches the target collection on the server, results load 25 at a time with **Show more**, and the arrow keys and Enter choose a record. A chosen record shows its title with **Change**, **Clear** and a related-record link. A record’s **Referenced by** section lists incoming links. Collection renames update every incoming schema atomically; deleting a collection first requires removing any schema fields that target it.

CSV and Excel XLSX imports support worksheet selection, column mapping, previews and row-level validation. Choose **Create or update by a unique field** and select a string/number field such as SKU or email. Matching rows keep their IDs and unmapped values; empty cells preserve existing values. Keys are case-sensitive, numbers match by exact decimal value, and duplicate matching keys in either the file or collection stop the entire import. Preview reports new, updated and unchanged rows. A commit revalidates current data and writes only changes. Imports are limited to 50,000 rows and 50 MB per file and commit atomically: validation failures write no records.

Open **Export records** to choose CSV or JSON. **Export current view** downloads all matching records across pages using visible columns. **Export selected** downloads the selected records. The Export panel can also include columns from related records: for each reference field, tick fields of the collection it points to. CSV adds columns named like `product.name` next to the reference column, with lists of references exported as a JSON list of values. JSON adds a `related` object per record with each related record's `id` and the chosen fields (`"missing": true` if it no longer exists). The API takes `"related": [{"field": "product", "columns": ["name", "price"]}]`; a collection-scoped API key cannot export columns from other collections. CSV uses API field names as headers and JSON-encodes structured values; formula-like text gets an apostrophe prefix for spreadsheet safety. JSON keeps exact values, IDs, timestamps and calculation errors. Exports have explicit limits of 10,000 records / 25 MB and selected exports allow up to 100 IDs. Narrow the view when exceeding a limit. CSV exports can be imported with matching headers; use JSON for exact value preservation and for backups that include metadata.

Database migrations run transactionally on server startup and backfill search indexes, relationship links and baseline history. Rebuild the UI and restart the backend to load these features. No manual database reset is required.

See [calculated fields and imports](docs/CALCULATED_FIELDS.md) for supported functions, examples and compatibility limits, and [the development code review](docs/CODE_REVIEW.md) for review status and verification.

## Summaries and charts

Switch the records toolbar from **Table** to **Summary** to group records and calculate totals. Search and filters apply first, then:

- **Group records by** up to three fields: text, number, true/false, date, reference or calculated fields. Dates can be grouped by day, month or year. References group by the related record and show its title. Text keeps its capitalisation, so `Barnehage1` and `barnehage1` are separate groups. Records without a value form an "(empty)" group, listed last.
- **Values** (up to 20): sum, average, minimum or maximum of a number or calculated field (minimum and maximum also work on dates), count of records or of non-empty values in a field, count of unique values, or a **formula** over the other values, such as `=[total_revenue]/3.14` or `=[total_revenue]*[average_employees]`. Formulas use the same functions as calculated fields and are worked out per group and for the totals row. A formula that fails, for example dividing by zero, shows "Error" for that group only.
- **Sort** groups by any group field or value, ascending or descending.
- **Chart**: a bar or line chart of up to four values on one shared axis, with a legend for two or more values and a tooltip per group on hover or keyboard focus. If the values differ greatly in size, the chart says so. Chart them separately in that case, since one axis makes the smaller ones look flat.

The table lists every group with its record count and an **All records** totals row. **View records →** opens the table filtered to that group. **Export summary CSV** downloads the table. Up to 1,000 groups are shown; a summary with more than 10,000 groups is refused, so group more coarsely or filter first.

A view saved in Summary mode keeps its summary and opens in Summary mode. Field renames update saved summaries.

Over the API, summaries are reads (any role, and collection-scoped keys for their collection):

```bash
curl -X POST http://127.0.0.1:8080/api/v1/collections/$ID/summary \
  -H "Authorization: Bearer $VULSE_API_KEY" -H 'content-type: application/json' \
  -d '{"query":{"filters":[]},"summary":{
        "group_by":[{"field":"name"}],
        "measures":[
          {"name":"total_revenue","op":"sum","field":"revenue"},
          {"name":"average_employees","op":"average","field":"employees"},
          {"name":"per_employee","op":"formula","formula":"=[total_revenue]/[average_employees]"}],
        "sort":"total_revenue","descending":true}}'
```

`op` is one of `sum`, `average`, `min`, `max`, `count` (without `field` it counts records), `count_unique` or `formula`. A date group takes `"bucket": "day" | "month" | "year"`. The response has `rows` (each with `group` values, readable `labels`, `count`, `values` and any formula `errors`), a `totals` row, `group_count` and `truncated`. `GET /api/v1/collections/{id}/views/{view_id}/summary` runs a saved view's summary. Public collections also accept `POST /api/v1/public/{collection}/summary` without credentials, from any site.

## SQL

**SQL** in the top bar runs Vulse SQL statements on one collection at a time:

```sql
SELECT kommune, COUNT(*) AS records, ROUND(AVG(aarsverk), 2) AS average
FROM aarsmelding
WHERE aar >= 2023 AND eiertype = 'Privat'
GROUP BY kommune
ORDER BY records DESC
LIMIT 20

UPDATE aarsmelding SET eiertype = 'Privat' WHERE eiertype = 'privat'

DELETE FROM aarsmelding WHERE aar < 2020
```

- **Statements:** `SELECT` (columns or `*`, `WHERE`, `GROUP BY`, `ORDER BY` by expression, alias or position, `LIMIT`, `OFFSET`), `UPDATE … SET field = expression, … [WHERE …]` and `DELETE FROM … [WHERE …]`. One statement at a time; there are no joins, `INSERT` or schema changes.
- **Names:** a statement's fields are the collection's API field names, plus `id`, `created_at` and `updated_at`. Collection names may contain hyphens (`FROM pensjon-2024`). Quote names that are keywords with double quotes (`"order"`). Text values use single quotes.
- **Conditions:** `= <> < <= > >=`, `AND`, `OR`, `NOT`, `IN (…)`, `BETWEEN … AND …`, `LIKE` (`%` and `_`, ignoring case), `IS NULL` and `IS NOT NULL` (blank text counts as empty). Numbers and numeric text compare as numbers; `=` on text is case-sensitive. A comparison with an empty value is never true, so `WHERE kommune <> 'Oslo'` skips records without a kommune.
- **Functions:** `COUNT(*)`, `COUNT`, `SUM`, `AVG`, `MIN` and `MAX` over groups, `COALESCE`, `IFNULL`, `LENGTH`, `SUBSTR`, and the formula functions such as `ROUND`, `UPPER`, `YEAR` or `IF`. Calculated fields can be read, including ones that read other collections, but not set.
- **Changes are previewed:** running an `UPDATE` or `DELETE` changes nothing. It shows how many records will change and the first 20, with before → after values for updates, and any records that would become invalid (which block the statement). After you confirm, exactly the previewed records are changed in one transaction; if any of them changed in the meantime, you are asked to preview again. Updates are validated like edits in the table and keep each record's previous version in its history; deleted records go to the collection's Trash and can be restored. Records that other records still link to cannot be deleted. One statement can change up to 50,000 records.
- **Who can run what:** viewers can run `SELECT`; editors and admins can also run `UPDATE` and `DELETE`. Collection-scoped API keys only reach their collection.
- **Editor:** keywords, functions, text and numbers are highlighted. Suggestions follow what you type: collection names after `FROM` or `UPDATE`, then that collection's fields, keywords and functions (↑ ↓ to choose, Enter or Tab to insert). Ctrl + Enter runs. Templates fill in common statements with the chosen collection's fields, and the statement is kept when you reload.
- **API:** `POST /api/v1/sql` with `{"sql": "…"}` returns `columns`, `rows` (up to 1,000) and `total` for a `SELECT`, or a preview with `affected`, `changes`, `problems` and a `confirmation`. Send the same statement with `"confirm": "<confirmation>"` to apply it.

## Rich text and collection API reference

Portable Text fields use a locally bundled [Tiptap](https://tiptap.dev/docs/editor/introduction) editor: headings, bold, italic, underline, strikethrough, inline code, quotes, nested lists (six levels), image uploads, and undo/redo. Paste and drop images directly into the document. Existing Portable Text remains the storage and API format; no database migration is needed. The supported subset excludes link annotations, tables, and code blocks. Trunk builds the JavaScript bundle through a pre-build hook after `npm ci`.

**Insert block** adds accordions, repeatable text groups, and iframe embeds. Accordion headings and initial collapsed state are editable; each repeatable entry has an optional label and rich text, with add/remove/reorder controls. Images and other blocks can be nested inside these containers. Embeds accept absolute HTTP/HTTPS URLs, recognize YouTube/Vimeo share links, and provide title/height settings and an optional sandboxed preview. A provider must allow embedding for its preview to work. See [the stored block shapes and extension guide](docs/RICH_TEXT_BLOCKS.md).

Each collection’s API tab displays its routes, authentication requirements, public-read availability, and copyable curl examples generated from the saved schema. Calculated fields are omitted from writes. The tab explains query/views, partial and bulk writes, optimistic version checks, history/Trash/backlinks, exports, import updates, and asset uploads.

Verify the editor adapter with `npm --prefix crates/web test`, and shared/server behavior with `cargo test --workspace`.

The browser regression check uses a system Chromium (`CHROMIUM_PATH` overrides `/usr/bin/chromium`) and an isolated server on port 8098. Start it in one terminal after building the UI:

```bash
BIND=127.0.0.1:8098 VULSE_DB_PATH=/tmp/vulse-editor-tests.db ASSET_DIR=/tmp/vulse-editor-tests-assets \
  VULSE_SETUP_TOKEN=vulse-browser-tests-setup cargo run -p vulse-server
```

Then run `npm --prefix crates/web run test:browser`. The first suite against a fresh database uses the setup token to create a bootstrap admin (`admin@vulse.test`). Each suite then invites its own admin account through the accept page, and appends a run suffix to its collection names because names are shared across the workspace. If you pass a different `VULSE_SETUP_TOKEN` to the server, export the same value for the test command. The suite checks existing content preservation, formatting, keyboard navigation, nested lists, pending uploads, save/reload, API examples, clipboard copying, public reads and mobile layout. `VULSE_TEST_URL` can override the test address.

Run `npm --prefix crates/web run test:blocks` against the same isolated server to check structured block editing, nesting, nested image uploads, iframe previews, history, API round trips and mobile layout.

Calculated fields now start in a guided builder with named, typed inputs and a live preview. Choose sample values or an existing record without modifying saved data. The advanced editor supports field suggestions, function help, insertion at the cursor, and wrapping selected expressions. Run `npm --prefix crates/web run test:calculations` against the isolated test server above to check this workflow.

Run `npm --prefix crates/web run test:workspace` against the isolated server to verify the six usability priorities, draft recovery, revision restore, relationship navigation, import updates, downloads and mobile layout. Each suite creates its own account and collections; none should run against a production database.
