import { test } from "node:test";
import assert from "node:assert/strict";
import { toTiptap, toPortable } from "./portable-text.js";
import { embedUrl } from "./custom-blocks.js";
const block = (key, style = "normal", text = "Hello", marks = []) => ({ _key: key, _type: "block", style, markDefs: [], children: [{ _key: `${key}_span`, _type: "span", text, marks }] });
test("preserves custom blocks, nested lists, formatting, empty entries and nested asset references", () => {
  const blocks = [{ _key: "accordion", _type: "accordion", title: "FAQ", collapsed: true, content: [
    block("answer", "h2", "Answer 😀", ["strong"]),
    { ...block("list"), listItem: "bullet", level: 1 },
    { ...block("nestedList"), listItem: "number", level: 2 },
    { _key: "group", _type: "repeatable", title: "Examples", items: [
      { _key: "entry1", label: "First", content: [{ _key: "image", _type: "image", asset: { _type: "reference", _ref: "550e8400-e29b-41d4-a716-446655440000" } }, block("text", "normal", "Combined marks", ["code", "strong"])] },
      { _key: "entry2", label: "", content: [] }
    ] },
    { _key: "embed", _type: "iframe", title: "Video", url: "https://example.com/embed", height: 400 }
  ] }];
  assert.deepEqual(toPortable(toTiptap(blocks, "collection")), blocks);
  const doc = toTiptap(blocks, "collection");
  doc.content.push(structuredClone(doc.content[0]));
  const duplicated = toPortable(doc);
  const keys = [];
  function walk(blocks) { for (const b of blocks) { keys.push(b._key); if (b.content) walk(b.content); for (const i of b.items || []) { keys.push(i._key); walk(i.content); } } }
  walk(duplicated);
  assert.equal(keys.length, new Set(keys).size);
  assert.equal(duplicated[1].content[4]._type, "iframe");
});
test("normalizes video share links and rejects unsafe iframe URLs", () => {
  assert.equal(embedUrl("https://youtu.be/dQw4w9WgXcQ?t=30"), "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?start=30");
  assert.equal(embedUrl("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ");
  assert.equal(embedUrl("https://vimeo.com/12345"), "https://player.vimeo.com/video/12345");
  assert.equal(embedUrl("https://example.com/embed?a=b"), "https://example.com/embed?a=b");
  for (const url of ["javascript:alert(1)", "data:text/html,test", "//example.com", "https:example.com", "https://u:p@example.com", "https://example.com\\@evil.test/", "https://example.com/\n"]) assert.throws(() => embedUrl(url));
});
test("preserves existing spans, unicode, headings, quotes, lists, and asset references", () => {
  const blocks = [
    block("one", "h1", "Hello \u{1F600}", ["strong", "em"]),
    block("two", "blockquote", "Quote"),
    { ...block("three", "h2", "Listed heading", ["code"]), listItem: "number", level: 1 },
    { _key: "image", _type: "image", asset: { _type: "reference", _ref: "550e8400-e29b-41d4-a716-446655440000" } }
  ];
  assert.deepEqual(toPortable(toTiptap(blocks, "collection")), blocks);
});
test("round trips mixed nested lists and paragraphs", () => {
  const blocks = [{ ...block("a"), listItem: "bullet", level: 1 }, { ...block("b"), listItem: "number", level: 2 }, { ...block("c"), listItem: "bullet", level: 3 }, { ...block("d"), listItem: "number", level: 2 }, { ...block("e"), listItem: "bullet", level: 1 }, block("f")];
  assert.deepEqual(toPortable(toTiptap(blocks, "collection")), blocks);
});
test("converts edited marks, hard breaks, and duplicated block keys into valid Portable Text", () => {
  const doc = { type: "doc", content: [{ type: "paragraph", attrs: { portableKey: "same" }, content: [{ type: "text", text: "Bold \u{1F600}", marks: [{ type: "bold" }, { type: "underline" }] }, { type: "hardBreak" }, { type: "text", text: "Next", marks: [{ type: "strike" }] }] }, { type: "paragraph", attrs: { portableKey: "same" }, content: [{ type: "text", text: "Split" }] }] };
  const blocks = toPortable(doc);
  assert.notEqual(blocks[0]._key, blocks[1]._key);
  assert.equal(blocks[0].children.map((s) => s.text).join(""), "Bold \u{1F600}\nNext");
  assert.deepEqual(blocks[0].children[0].marks, ["strong", "underline"]);
  assert.deepEqual(blocks[0].children.at(-1).marks, ["strike"]);
  assert.deepEqual(toPortable(toTiptap(blocks, "collection")), blocks);
});
test("empty editors remain empty and external image nodes are omitted", () => {
  assert.deepEqual(toPortable(toTiptap([], "collection")), []);
  assert.deepEqual(toPortable({ type: "doc", content: [{ type: "image", attrs: { src: "https://example.test/image.png" } }] }), []);
});
test("preserves newline marks, adjacent matching spans, and maximum key lengths", () => {
  const original = block("a".repeat(80), "normal", "First\nSecond", ["strong", "code"]);
  original.children.push({ _key: "last", _type: "span", text: " third", marks: ["strong", "code"] });
  assert.deepEqual(toPortable(toTiptap([original], "collection")), [original]);
  const doc = toTiptap([original], "collection");
  doc.content[0].content.at(-1).text += " changed";
  assert(toPortable(doc)[0].children.every((s) => s._key.length <= 80));
});
