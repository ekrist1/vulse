# Structured rich text blocks

In a Portable Text field, choose **Insert block** and select **Accordion**, **Repeatable text**, or **Iframe embed**. Blocks are inserted at the cursor. When the cursor is inside a list or quote, insertion places the block after that list or quote within its enclosing container. All controls participate in undo/redo.

Accordions have an editable heading and rich text body. **Collapse / Expand** previews the current editing state; **Start collapsed** sets the saved initial state. Press Enter in the heading to enter the body. Repeatable groups have a title and 1–100 entries, each with an optional label and rich text body. Add, move and remove entries using their controls. The last entry cannot be removed; remove the whole group instead. Images and custom blocks can be nested inside accordion bodies and repeatable entries, up to six container levels.

Iframe embeds accept a full HTTP/HTTPS URL, including YouTube and Vimeo share links. Enter a descriptive title and height between 160 and 1200 pixels. **Load preview** requests the embedded page; previews are hidden on reload. **Edit embed** changes settings. Providers can disallow iframes through their own headers, or require capabilities unavailable in the sandbox. Embeds cannot use inline HTML, `srcdoc`, scripts as URLs, credentials, or arbitrary iframe permissions. The preview sandbox allows scripts, forms and presentation, but omits same-origin access and top navigation; its referrer policy is `strict-origin-when-cross-origin`.

## Storage and APIs

These nodes extend the existing Portable Text array. The server validates their shapes, URL schemes, titles (at most 200 characters), entry counts, nesting and key uniqueness. Image references inside every nested body are checked against assets in the collection. Existing text and image block formats are unchanged. No database migration is required. Both authenticated and public document reads return these blocks unchanged; API consumers must add renderers for the new types.

```json
[
  {
    "_key": "faq",
    "_type": "accordion",
    "title": "Frequently asked questions",
    "collapsed": true,
    "content": [
      {
        "_key": "answer",
        "_type": "block",
        "style": "normal",
        "markDefs": [],
        "children": [{"_key": "answer_span", "_type": "span", "text": "The answer.", "marks": []}]
      }
    ]
  },
  {
    "_key": "examples",
    "_type": "repeatable",
    "title": "Examples",
    "items": [{"_key": "example_1", "label": "First example", "content": []}]
  },
  {
    "_key": "video",
    "_type": "iframe",
    "url": "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
    "title": "Demo video",
    "height": 360
  }
]
```

`content` is another Portable Text array and may be empty. Every block and repeatable entry has a globally unique `_key` across the field, at most 80 ASCII letters, digits, underscores or hyphens. Span keys retain the existing rules.

## Adding another block type

1. Define a Tiptap node and editing controls in `crates/web/js/custom-blocks.js`, following the registered extensions and `contentDOM` pattern. Keep settings/control DOM outside editable content. Add its name to `blockTypes` for persistent keys.
2. Add its insertion choice/template in `rich-editor.js` and `custom-blocks.js`.
3. Define both directions of its Portable Text mapping in `portable-text.js`; preserve keys and nested rich text rather than converting bodies to HTML strings.
4. Add strict server/shared-model validation in `crates/model/src/portable_text.rs`, including recursive asset collection where relevant. Unknown types remain rejected until registered here. The legacy Rust adapter retains validated custom blocks as `EditorBlock::Custom`.
5. Add adapter/model behavior coverage and extend `scripts/blocks-check.cjs` with a browser save/reload scenario. Run `npm --prefix crates/web test`, `cargo test --workspace`, `trunk build --config crates/web/Trunk.toml`, and `npm --prefix crates/web run test:blocks` against an isolated server.

Custom blocks are implemented with [Tiptap's JavaScript node views](https://tiptap.dev/docs/editor/extensions/custom-extensions/node-views/javascript) and [node schema API](https://tiptap.dev/docs/editor/extensions/custom-extensions/create-new/node). They do not introduce a runtime CDN or framework dependency.
