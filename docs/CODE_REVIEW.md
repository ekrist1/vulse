# Vulse development review

Reviewed the model, Axum handlers, database wrapper/store, Yew forms/pages and the three supplied screenshots. The shared model and API authorization boundaries provide a useful foundation. The main UI problem was that configuration dominated the collection screen while record values were hidden.

## Review findings and current status

- **Collection rename integrity: fixed.** All incoming collection blueprints, including self references and reference arrays, are retargeted in the same transaction as the rename. Relationship targets still use collection names in the public schema for compatibility.
- **Dangling references on deletion: fixed for new operations.** A transactional relationship index tracks document links, direct assets, asset arrays, and images nested inside rich text blocks. Deleting a referenced record or asset returns 409 and the source record IDs. A bulk delete may remove records that reference one another when all sources are in the selected batch. Undoing duplicates uses the same guard. Collection deletion is blocked by incoming schema references, even when no records use them.
- **Validation/write race: fixed.** Ordinary writes, schema changes, import commits, bulk actions and revision restores read, validate and persist in one write transaction. Optional version tokens protect API edits; the admin sends them for record and schema saves.
- **Blocking password work: fixed.** Argon2 hashing and verification run on blocking workers with four shared permits per application instance. Saturation returns 429; credentials remain valid after capacity is released. Production deployments should also configure request throttling at their ingress.
- **Full collection downloads: removed from the record table and reference pickers.** Queries search indexed values on the server and fetch only the visible page. Sorting has an ID tie breaker. Offset pagination remains; use a cursor design if navigation must stay stable under frequent concurrent insertions.
- **Draft recovery: implemented.** Record and schema drafts persist per account and collection on the device, including incomplete number/JSON inputs. Recovery is explicit, old versions are conflict checked, and record drafts can merge changed fields onto the latest record. Persistent record history provides differences, restore controls and Trash.

Existing dangling references from operations performed before these changes may require correction. This is a development review, not a penetration test or an Excel conformance certification. Remote Turso execution is implemented through the existing database abstraction; the checks below exercise local Turso.

## Implemented improvements

- A responsive workspace overview with searchable collection cards, summary counts and a deliberate new-collection flow.
- Records as the default collection view. A horizontally scrollable table shows all fields, calculated values/errors, server search and page controls. Fields, Import data, API, Trash and Settings have separate views.
- A compact field editor, unique generated field names and valid default reference/array targets. Previously an ordinary string array could get an invalid `collection` setting, and a newly selected reference had an empty target despite the picker appearing to choose the current collection.
- Record controls have accessible names; checkboxes retain their intrinsic width. Rich editors use a container instead of an invalid enclosing label. Focus styling, empty states, save feedback and responsive layouts improve navigation.
- Confirmation for deleting collections and records; pending state for collection/record saves and imports.
- Fifty deterministic Excel-style functions, formula/input pickers, schema validation, dependency checks, live editing previews and freshly calculated values in authenticated and public API responses. Records refresh every 15 seconds while the Records view is open.
- CSV/XLSX upload, worksheet selection, column mapping, type conversion, create/update previews, row/field validation errors and transactional all-or-nothing writes. File/row/column/archive/coordinate limits are enforced. Legacy XLS should first be saved as XLSX.

- Tiptap replaces the block-by-block Portable Text form with one document surface, formatting controls, nested lists, image paste/drop/upload and undo/redo. The bridge retains Portable Text storage, existing span keys for unchanged content, and asset IDs. It maps pending image positions through edits so upload completion does not overwrite newer text.
- A collection API tab shows authenticated and public routes, collection/account key scopes, pagination and validation details, and copyable curl/JSON examples generated from the saved blueprint.

## Six usability priorities implemented

1. Display labels, descriptions, examples and configurable record titles, preserving stable API keys.
2. Persisted views with filters, sorting, grouping and visible columns; server search, totals and pagination.
3. Inline scalar editing, visible-page selection, transactional bulk update/duplicate/delete, and conflict-aware undo.
4. Recoverable record and schema drafts, visible unsaved status, optimistic saves, revision differences/restore and Trash.
5. Searchable paginated reference pickers, readable related-record links, incoming backlinks, atomic retargeting on rename and deletion guards.
6. CSV/XLSX imports that create or update by a unique scalar field, explicit action previews/counts, duplicate-key validation and unchanged-row detection; filtered-view and selected-row exports as CSV/JSON.

## Verification

- Workspace Rust tests cover all 50 formula functions, validation, authorization, source/formula changes, server querying and saved views, metadata/title semantics, batch rollback/undo/conflicts, draft save version tokens, history/Trash restores, relationship rename/delete/undo guards, scope-safe backlinks, nested image deletion guards, upsert counts/keys/atomicity, export filtering/selection/CSV quoting, bounded authentication work, and legacy database backfills.
- JavaScript adapter checks preserve Portable Text, span keys, marks, nested lists/images/custom blocks and safe embed URLs.
- Trunk builds the Wasm application and locally bundled Tiptap extensions.
- `npm --prefix crates/web run test:workspace` checks labels/title/help, saved view persistence, inline and bulk edits/undo, record/schema draft recovery, revision restore, searchable references/navigation/backlinks, import updates and CSV/JSON downloads, history refresh after saves, and compact mobile/desktop layouts. The other browser suites are `test:browser`, `test:calculations` and `test:blocks`.
- All 40 workspace Rust tests pass. Clippy completes with style warnings; strict warning-free linting is not currently configured.

See the README for usage, limits and how to run browser checks against an isolated server.
