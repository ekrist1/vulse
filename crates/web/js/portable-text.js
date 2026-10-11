// Portable Text remains the API/storage format; Tiptap owns the editing surface.
const toMark = { strong: "bold", em: "italic", code: "code", underline: "underline", strike: "strike" };
const fromMark = Object.fromEntries(Object.entries(toMark).map(([a, b]) => [b, a]));
let sequence = 0;
export const newKey = () => `pt_${Date.now().toString(36)}_${(sequence++).toString(36)}`;
function inline(block) {
  return block.children.flatMap((span) => span.text.split("\n").flatMap((text, i) => [
    ...i ? [{ type: "hardBreak", marks: (span.marks || []).map((m) => ({ type: toMark[m] })) }] : [],
    ...text ? [{ type: "text", text, marks: (span.marks || []).map((m) => ({ type: toMark[m] })) }] : []
  ]));
}
function textNode(block, inList = false) {
  const type = !inList && /^h[1-3]$/.test(block.style) ? "heading" : "paragraph";
  const node = { type, attrs: {
    portableKey: block._key,
    portableStyle: inList ? block.style : null,
    portableOriginal: JSON.stringify(block),
    ...type === "heading" ? { level: Number(block.style[1]) } : {}
  }, content: inline(block) };
  return !inList && block.style === "blockquote" ? { type: "blockquote", content: [node] } : node;
}
export function toTiptap(blocks, collectionId) {
  const content = [];
  const stack = [];
  for (const block of blocks) {
    if (block._type === "image") {
      stack.length = 0;
      content.push({ type: "image", attrs: {
        portableKey: block._key,
        assetId: block.asset._ref,
        src: `/api/v1/collections/${collectionId}/assets/${block.asset._ref}/content`
      } });
    } else if (["iframe", "accordion", "repeatable"].includes(block._type)) {
      stack.length = 0;
      const attrs = { portableKey: block._key, title: block.title };
      if (block._type === "iframe") content.push({ type: "embed", attrs: { ...attrs, url: block.url, height: block.height } });
      if (block._type === "accordion") content.push({ type: "accordion", attrs: { ...attrs, collapsed: block.collapsed }, content: toTiptap(block.content, collectionId).content });
      if (block._type === "repeatable") content.push({ type: "repeatable", attrs, content: block.items.map((item) => ({ type: "repeatableItem", attrs: { portableKey: item._key, label: item.label }, content: toTiptap(item.content, collectionId).content })) });
    } else if (block.listItem) {
      const depth = Math.min(block.level || 1, stack.length + 1, 6);
      stack.length = Math.min(stack.length, depth);
      const type = block.listItem === "number" ? "orderedList" : "bulletList";
      if (!stack[depth - 1] || stack[depth - 1].type !== type) {
        const list = { type, content: [] };
        const parent = depth === 1 ? content : stack[depth - 2].content.at(-1).content;
        parent.push(list);
        stack[depth - 1] = list;
      }
      stack[depth - 1].content.push({ type: "listItem", content: [textNode(block, true)] });
    } else {
      stack.length = 0;
      content.push(textNode(block));
    }
  }
  return { type: "doc", content: content.length ? content : [{ type: "paragraph" }] };
}
function spans(node) {
  const result = [];
  for (const child of node.content || []) {
    const text = child.type === "hardBreak" ? "\n" : child.text || "";
    const marks = (child.marks || []).map((m) => fromMark[m.type]).filter(Boolean).sort();
    const last = result.at(-1);
    if (last && JSON.stringify(last.marks) === JSON.stringify(marks)) last.text += text;
    else result.push({ _key: newKey(), _type: "span", text, marks });
  }
  return result.length ? result : [{ _key: newKey(), _type: "span", text: "", marks: [] }];
}
export function toPortable(doc) {
  let blocks = [];
  const used = new Set();
  const unique = (candidate) => {
    const value = candidate && !used.has(candidate) ? candidate : newKey();
    used.add(value);
    return value;
  };
  function nested(node) {
    const outer = blocks;
    blocks = [];
    for (const child of node.content || []) visit(child);
    const result = empty(blocks);
    blocks = outer;
    return result;
  }
  function empty(value) {
    return value.length === 1 && value[0]._type === "block" && value[0].style === "normal" && !value[0].listItem && value[0].children.every((s) => !s.text) ? [] : value;
  }
  function visit(node, context = {}) {
    if (node.type === "embed") {
      blocks.push({ _key: unique(node.attrs.portableKey), _type: "iframe", url: node.attrs.url, title: node.attrs.title, height: node.attrs.height });
    } else if (node.type === "accordion") {
      const key = unique(node.attrs.portableKey);
      blocks.push({ _key: key, _type: "accordion", title: node.attrs.title, collapsed: node.attrs.collapsed, content: nested(node) });
    } else if (node.type === "repeatable") {
      const key = unique(node.attrs.portableKey);
      const items = (node.content || []).map((item) => ({ _key: unique(item.attrs.portableKey), label: item.attrs.label, content: nested(item) }));
      blocks.push({ _key: key, _type: "repeatable", title: node.attrs.title, items });
    } else if (node.type === "image") {
      if (node.attrs?.assetId) blocks.push({ _key: unique(node.attrs.portableKey), _type: "image", asset: { _type: "reference", _ref: node.attrs.assetId } });
    } else if (node.type === "paragraph" || node.type === "heading") {
      const style = node.type === "heading" ? `h${node.attrs.level}` : context.quote ? "blockquote" : node.attrs?.portableStyle || "normal";
      const block = { _key: unique(node.attrs?.portableKey), _type: "block", style, markDefs: [], children: spans(node) };
      block.children.forEach((span, index) => span._key = `${block._key.slice(0, 64)}_s${index}`);
      if (context.list) {
        block.listItem = context.list;
        block.level = context.depth;
      }
      // Preserve existing span keys and optional properties for unchanged blocks.
      const original = node.attrs?.portableOriginal ? JSON.parse(node.attrs.portableOriginal) : null;
      const shape = (b) => {
        const children = [];
        for (const span of b.children) {
          const marks = [...span.marks || []].sort();
          const last = children.at(-1);
          if (last && JSON.stringify(last.marks) === JSON.stringify(marks)) last.text += span.text;
          else children.push({ text: span.text, marks });
        }
        return JSON.stringify({ style: b.style, listItem: b.listItem, level: b.listItem ? b.level || 1 : void 0, children });
      };
      if (original && original._key === block._key && shape(original) === shape(block)) blocks.push(original);
      else blocks.push(block);
    } else if (node.type === "bulletList" || node.type === "orderedList") {
      for (const child of node.content || []) visit(child, { list: node.type === "bulletList" ? "bullet" : "number", depth: (context.depth || 0) + 1 });
    } else {
      for (const child of node.content || []) visit(child, { ...context, quote: context.quote || node.type === "blockquote" });
    }
  }
  visit(doc);
  return empty(blocks);
}
