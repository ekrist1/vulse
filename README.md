# Vulse

Vulse stores JSON documents in named collections. Each collection has a blueprint that describes its fields, and each document is checked against that blueprint. A collection is either public or private. Public documents can be read without an account. Private ones need a signed-in session or an API key.

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

The server listens on `127.0.0.1:8080`. Open that address in a browser. The admin is served from `crates/web/dist`. If that directory is missing, the API still runs and the site routes are absent.

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

Set both Turso variables, or neither. The remote database should be created with `turso db create --tursodb`, because the server uses the current Turso Rust engine.

## Adding users

There is no seeded account. Create the first one from the admin at [http://127.0.0.1:8080/register](http://127.0.0.1:8080/register), or with the API:

```bash
curl -c cookies.txt -H 'content-type: application/json' \
  -d '{"email":"ada@example.com","handle":"ada","password":"correct-horse"}' \
  http://127.0.0.1:8080/api/v1/auth/register
```

The handle is 2–32 characters of lowercase letters, digits, or hyphens. The password is 8–128 characters. Email and handle must be unique. Register and login set an HttpOnly `vulse_session` cookie that the admin uses.

API keys are created while signed in, from the API keys page or `POST /api/v1/api-keys`. The raw secret (`vulse_…`) is returned once. Send it as `Authorization: Bearer vulse_…`. A key can cover the whole account or a single collection.

Public reads do not use the cookie or the key:

```text
GET /api/v1/public/{handle}/{collection}
GET /api/v1/public/{handle}/{collection}/{document_id}
GET /api/v1/public/{handle}/{collection}/assets/{asset_id}
```

A missing collection and a private collection both respond with 404.

## Architecture

```text
crates/model    shared types and validation, compiled for the server and for wasm
crates/server   Axum API, Turso access, sessions, and the file store
crates/web      Yew admin, built with Trunk
```

The admin and other clients talk to Axum. Axum writes rows to Turso and file bytes to disk. Document and asset ids in the database are text UUIDs. The original upload name is metadata only. The file on disk is named `{owner id}/{asset id}`.

A blueprint is an ordered field list: string, number, boolean, free-form JSON, Portable Text, asset, reference, or an array of string, number, asset, or reference. The server rejects unknown document keys and checks that references and assets exist. Saving a blueprint rechecks every document already in the collection. If any would become invalid, the change is refused and the response lists those document ids.

Passwords are hashed with Argon2id. API key secrets are stored as SHA-256 hashes. Collection-scoped keys can read and write documents and assets in that collection only.

## Collection workspace

The admin opens collections on a searchable record table, with separate Fields, Import data, API, Trash and Settings views. Record values refresh every 15 seconds while that view is open. A calculated field uses a formula such as `=ROUND([price] * [quantity], 2)` and supports 50 Excel-style functions. Results are derived on the server from current data and formulas, including public API reads.

Fields can have a display label, description and example while their API names remain stable. In Fields, choose a title field for readable record links and reference choices. Open **Customize view** to set filters, sorting, grouping, columns and the saved view name. These controls stay collapsed until needed. Saved views preserve typed filters, sort order, grouping and visible columns. Search and pagination run on indexed server values; each refresh fetches the visible page.

Click a text, number or boolean cell to edit it. Enter saves and Escape cancels. Select rows on the current page to update one field, duplicate, delete or export them. **Undo last action** reverses a batch within 24 hours if its records have not changed. Referenced targets cannot be deleted until incoming links are removed; a batch may delete records that only reference one another.

Unsaved record and collection drafts are stored under your account on this device. On returning, choose Restore or Discard. Older record drafts can apply only their changed fields to the latest record; when both edits change the same field, the explicit merge keeps the draft value. Review older schema drafts against the current fields before choosing **Use latest collection version**. Incomplete number/JSON inputs are retained. Storage failures are shown visibly. Saved record history includes field differences and restore controls, and **Trash** recovers deleted record IDs. Existing data receives a baseline version during migration; history cannot reconstruct earlier edits. Restores validate against the current schema, references and files.

Reference fields search their target collection in pages of 25 with readable titles and a related-record link. A record’s **Referenced by** section lists incoming links. Collection renames update every incoming schema atomically; deleting a collection first requires removing any schema fields that target it.

CSV and Excel XLSX imports support worksheet selection, column mapping, previews and row-level validation. Choose **Create or update by a unique field** and select a string/number field such as SKU or email. Matching rows keep their IDs and unmapped values; empty cells preserve existing values. Keys are case-sensitive, numbers match by exact decimal value, and duplicate matching keys in either the file or collection stop the entire import. Preview reports new, updated and unchanged rows. A commit revalidates current data and writes only changes. Imports are limited to 1,000 rows per batch and commit atomically: validation failures write no records.

Open **Export records** to choose CSV or JSON. **Export current view** downloads all matching records across pages using visible columns. **Export selected** downloads the selected records. CSV uses API field names as headers and JSON-encodes structured values; formula-like text gets an apostrophe prefix for spreadsheet safety. JSON keeps exact values, IDs, timestamps and calculation errors. Exports have explicit limits of 10,000 records / 25 MB and selected exports allow up to 100 IDs. Narrow the view when exceeding a limit. CSV exports can be imported with matching headers; use JSON for exact value preservation and for backups that include metadata.

Database migrations run transactionally on server startup and backfill search indexes, relationship links and baseline history. Rebuild the UI and restart the backend to load these features. No manual database reset is required.

See [calculated fields and imports](docs/CALCULATED_FIELDS.md) for supported functions, examples and compatibility limits, and [the development code review](docs/CODE_REVIEW.md) for review status and verification.

## Rich text and collection API reference

Portable Text fields use a locally bundled [Tiptap](https://tiptap.dev/docs/editor/introduction) editor: headings, bold, italic, underline, strikethrough, inline code, quotes, nested lists (six levels), image uploads, and undo/redo. Paste and drop images directly into the document. Existing Portable Text remains the storage and API format; no database migration is needed. The supported subset excludes link annotations, tables, and code blocks. Trunk builds the JavaScript bundle through a pre-build hook after `npm ci`.

**Insert block** adds accordions, repeatable text groups, and iframe embeds. Accordion headings and initial collapsed state are editable; each repeatable entry has an optional label and rich text, with add/remove/reorder controls. Images and other blocks can be nested inside these containers. Embeds accept absolute HTTP/HTTPS URLs, recognize YouTube/Vimeo share links, and provide title/height settings and an optional sandboxed preview. A provider must allow embedding for its preview to work. See [the stored block shapes and extension guide](docs/RICH_TEXT_BLOCKS.md).

Each collection’s API tab displays its routes, authentication requirements, public-read availability, and copyable curl examples generated from the saved schema. Calculated fields are omitted from writes. The tab explains query/views, partial and bulk writes, optimistic version checks, history/Trash/backlinks, exports, import updates, and asset uploads.

Verify the editor adapter with `npm --prefix crates/web test`, and shared/server behavior with `cargo test --workspace`.

The browser regression check uses a system Chromium (`CHROMIUM_PATH` overrides `/usr/bin/chromium`) and an isolated server on port 8098. Start it in one terminal after building the UI:

```bash
BIND=127.0.0.1:8098 VULSE_DB_PATH=/tmp/vulse-editor-tests.db ASSET_DIR=/tmp/vulse-editor-tests-assets cargo run -p vulse-server
```

Then run `npm --prefix crates/web run test:browser`. This creates a test account and checks existing content preservation, formatting, keyboard navigation, nested lists, pending uploads, save/reload, API examples, clipboard copying, public reads and mobile layout. `VULSE_TEST_URL` can override the test address.

Run `npm --prefix crates/web run test:blocks` against the same isolated server to check structured block editing, nesting, nested image uploads, iframe previews, history, API round trips and mobile layout.

Calculated fields now start in a guided builder with named, typed inputs and a live preview. Choose sample values or an existing record without modifying saved data. The advanced editor supports field suggestions, function help, insertion at the cursor, and wrapping selected expressions. Run `npm --prefix crates/web run test:calculations` against the isolated test server above to check this workflow.

Run `npm --prefix crates/web run test:workspace` against the isolated server to verify the six usability priorities, draft recovery, revision restore, relationship navigation, import updates, downloads and mobile layout. Each suite creates its own account; none should run against a production database.
