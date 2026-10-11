import { Editor, Extension } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import Image from "@tiptap/extension-image";
import Code from "@tiptap/extension-code";
import Placeholder from "@tiptap/extension-placeholder";
import { Plugin, AllSelection, TextSelection } from "@tiptap/pm/state";
import { toTiptap, toPortable, newKey } from "./portable-text.js";
import { blockTypes, customBlockExtensions, blockTemplate, openEmbedDialog } from "./custom-blocks.js";
const Portable = Extension.create({ name: "portable", addGlobalAttributes() {
  return [
    { types: ["paragraph", "heading"], attributes: { portableKey: { default: null, rendered: false }, portableOriginal: { default: null, rendered: false }, portableStyle: { default: null, parseHTML: (element) => ["normal", "h1", "h2", "h3", "blockquote"].includes(element.getAttribute("data-portable-style")) ? element.getAttribute("data-portable-style") : null, renderHTML: (attrs) => attrs.portableStyle ? { "data-portable-style": attrs.portableStyle } : {} } } },
    { types: ["image"], attributes: { portableKey: { default: null, rendered: false }, assetId: { default: null, rendered: false } } },
    { types: blockTypes, attributes: { portableKey: { default: null, rendered: false } } }
  ];
}, addProseMirrorPlugins() {
  return [new Plugin({ appendTransaction(transactions, oldState, newState) {
    if (!transactions.some((t) => t.docChanged)) return null;
    const used = new Set();
    const tr = newState.tr;
    newState.doc.descendants((node, pos) => {
      if (["paragraph", "heading", "image", ...blockTypes].includes(node.type.name)) {
        let value = node.attrs.portableKey;
        if (!value || used.has(value)) {
          value = newKey();
          tr.setNodeMarkup(pos, void 0, { ...node.attrs, portableKey: value });
        }
        used.add(value);
      }
    });
    return tr.docChanged ? tr : null;
  }, filterTransaction(transaction) {
    let valid = true;
    function walk(node, depth = 0, containers = 0) {
      const next = depth + (["bulletList", "orderedList"].includes(node.type.name) ? 1 : 0);
      const nested = containers + (["accordion", "repeatable"].includes(node.type.name) ? 1 : 0);
      if (next > 6 || nested > 6 || (node.type.name === "repeatable" && node.childCount > 100)) valid = false;
      node.forEach((child) => walk(child, next, nested));
    }
    walk(transaction.doc);
    return valid;
  } })];
} });
const canonical = (value) => JSON.stringify(value, (key, value2) => value2 && !Array.isArray(value2) && typeof value2 === "object" ? Object.fromEntries(Object.entries(value2).filter(([key2]) => key2 !== "_key").sort(([a], [b]) => a.localeCompare(b))) : value2);
function syncTextSelection(view) {
  // Native caret navigation can precede ProseMirror's selectionchange observer.
  // Read editable text positions before commands; leave node/all selections intact.
  const selection = window.getSelection();
  if (!selection?.anchorNode || !selection.focusNode || !view.dom.contains(selection.anchorNode) || !view.dom.contains(selection.focusNode)) return;
  const inControl = (node) => (node.nodeType === 1 ? node : node.parentElement)?.closest('[contenteditable="false"]');
  if (inControl(selection.anchorNode) || inControl(selection.focusNode)) return;
  const anchor = view.posAtDOM(selection.anchorNode, selection.anchorOffset);
  const head = view.posAtDOM(selection.focusNode, selection.focusOffset);
  if (view.state.doc.resolve(anchor).parent.isTextblock && view.state.doc.resolve(head).parent.isTextblock && (view.state.selection.anchor !== anchor || view.state.selection.head !== head)) {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, anchor, head)));
  }
}
function mount(element, collectionId, label, json, onChange) {
  const shell = document.createElement("div");
  shell.className = "rich-editor";
  const toolbar = document.createElement("div");
  toolbar.className = "rich-toolbar";
  toolbar.setAttribute("role", "toolbar");
  toolbar.setAttribute("aria-label", `${label} formatting`);
  const surface = document.createElement("div");
  const status = document.createElement("p");
  status.className = "rich-status";
  status.setAttribute("role", "status");
  const input = document.createElement("input");
  input.type = "file";
  input.accept = "image/*";
  input.hidden = true;
  shell.append(toolbar, surface, status, input);
  element.append(shell);
  let destroyed = false;
  let uploading = false;
  let bookmark = null;
  const buttons = [];
  const editor = new Editor({
    element: surface,
    extensions: [
      StarterKit.configure({ link: false, code: false, codeBlock: false, horizontalRule: false, heading: { levels: [1, 2, 3] } }),
      Code.extend({ excludes: "" }),
      Portable,
      ...customBlockExtensions((attrs, save) => openEmbedDialog(shell, attrs, save, () => editor.view.focus())),
      Image.extend({ parseHTML() {
        return [{ tag: "img[src]", getAttrs: (element2) => {
          const prefix = `/api/v1/collections/${collectionId}/assets/`;
          const url = new URL(element2.getAttribute("src"), window.location.origin);
          const id = url.pathname.startsWith(prefix) ? url.pathname.slice(prefix.length).replace(/\/content$/, "") : null;
          return url.origin === window.location.origin && /^[0-9a-f-]{36}$/i.test(id || "") && url.pathname.endsWith("/content") ? { assetId: id } : false;
        } }];
      } }).configure({ allowBase64: false }),
      Placeholder.configure({ placeholder: "Start writing\u2026" })
    ],
    content: toTiptap(JSON.parse(json), collectionId),
    editorProps: {
      attributes: { role: "textbox", "aria-label": label, "aria-multiline": "true" },
      // Collapse whole-document selection in state before the next keyboard event.
      handleKeyDown(view, event) {
        if (view.state.selection instanceof AllSelection && ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
          const selection = ["ArrowLeft", "Home"].includes(event.key) ? TextSelection.atStart(view.state.doc) : TextSelection.atEnd(view.state.doc);
          view.dispatch(view.state.tr.setSelection(selection).scrollIntoView());
          return true;
        }
        syncTextSelection(view);
        return false;
      },
      handlePaste(view, event) {
        const files = [...event.clipboardData?.files || []];
        if (files.some((f) => f.type.startsWith("image/"))) {
          upload(files.find((f) => f.type.startsWith("image/")));
          return true;
        }
        return false;
      },
      handleDrop(view, event) {
        const file = [...event.dataTransfer?.files || []].find((f) => f.type.startsWith("image/"));
        if (file) {
          event.preventDefault();
          upload(file);
          return true;
        }
        return false;
      }
    },
    onUpdate: ({ editor: editor2 }) => onChange(JSON.stringify(toPortable(editor2.getJSON()))),
    onTransaction: ({ transaction }) => {
      // Follow edits while an image upload is pending.
      if (bookmark) bookmark = bookmark.map(transaction.mapping);
      refresh();
    }
  });
  // This Yew bridge can focus synchronously, avoiding deferred focus after a toolbar action.
  function focusedChain() {
    syncTextSelection(editor.view);
    editor.view.focus();
    return editor.chain();
  }
  function refresh() {
    for (const { button, active, enabled } of buttons) {
      if (active) button.setAttribute("aria-pressed", String(active()));
      button.disabled = enabled ? !enabled() : false;
    }
  }
  function add(label2, text, command, active, enabled) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "rich-tool";
    button.textContent = text;
    button.title = label2;
    button.setAttribute("aria-label", label2);
    button.addEventListener("mousedown", (e) => e.preventDefault());
    button.onclick = command;
    toolbar.append(button);
    buttons.push({ button, active, enabled });
  }
  const style = document.createElement("select");
  style.setAttribute("aria-label", "Paragraph style");
  for (const [value, label2] of [["paragraph", "Paragraph"], ["1", "Heading 1"], ["2", "Heading 2"], ["3", "Heading 3"]]) {
    const o = document.createElement("option");
    o.value = value;
    o.textContent = label2;
    style.append(o);
  }
  style.onchange = () => {
    const value = style.value;
    const chain = focusedChain();
    if (value === "paragraph") chain.setParagraph().updateAttributes("paragraph", { portableStyle: null }).run();
    else chain.setHeading({ level: Number(value) }).updateAttributes("heading", { portableStyle: null }).run();
  };
  toolbar.append(style);
  editor.on("selectionUpdate", () => {
    style.value = editor.isActive("heading") ? String(editor.getAttributes("heading").level) : "paragraph";
    refresh();
  });
  for (const [label2, text, cmd, type] of [["Bold", "B", "toggleBold", "bold"], ["Italic", "I", "toggleItalic", "italic"], ["Underline", "U", "toggleUnderline", "underline"], ["Strikethrough", "S", "toggleStrike", "strike"], ["Inline code", "<>", "toggleCode", "code"], ["Bullet list", "\u2022 List", "toggleBulletList", "bulletList"], ["Numbered list", "1. List", "toggleOrderedList", "orderedList"], ["Quote", "\u275D", "toggleBlockquote", "blockquote"]]) add(label2, text, () => focusedChain()[cmd]().run(), () => editor.isActive(type));
  add("Undo", "\u21B6", () => focusedChain().undo().run(), null, () => editor.can().undo());
  add("Redo", "\u21B7", () => focusedChain().redo().run(), null, () => editor.can().redo());
  add("Add image", "Image", () => input.click(), null, () => !uploading);
  const insertMenu = document.createElement("div");
  insertMenu.className = "rich-insert-menu";
  insertMenu.hidden = true;
  const insertButton = document.createElement("button");
  insertButton.type = "button";
  insertButton.className = "rich-tool";
  insertButton.textContent = "+ Insert block";
  insertButton.setAttribute("aria-expanded", "false");
  function closeMenu() { insertMenu.hidden = true; insertButton.setAttribute("aria-expanded", "false"); }
  insertButton.addEventListener("mousedown", (event) => event.preventDefault());
  insertButton.onclick = () => { insertMenu.hidden = !insertMenu.hidden; insertButton.setAttribute("aria-expanded", String(!insertMenu.hidden)); };
  toolbar.append(insertButton);
  toolbar.after(insertMenu);
  shell.addEventListener("keydown", (event) => { if (event.key === "Escape") closeMenu(); });
  function insertBlock(block) {
    const key = newKey();
    block.attrs = { ...block.attrs, portableKey: key };
    syncTextSelection(editor.view);
    const position = editor.state.selection.$from;
    let after;
    // Keep block content out of list items; retain the enclosing custom container.
    for (let depth = 1; depth <= position.depth; depth++) {
      if (["bulletList", "orderedList", "blockquote"].includes(position.node(depth).type.name)) { after = position.after(depth); break; }
    }
    const content = [block, { type: "paragraph" }];
    const inserted = after === undefined ? focusedChain().insertContent(content).run() : focusedChain().insertContentAt(after, content).run();
    let blockPosition;
    editor.state.doc.descendants((node, pos) => { if (node.attrs.portableKey === key) blockPosition = pos; });
    if (!inserted || blockPosition === undefined) { status.textContent = "Blocks can be nested at most six levels."; return; }
    if (block.type !== "embed") editor.view.dispatch(editor.state.tr.setSelection(TextSelection.near(editor.state.doc.resolve(blockPosition + 1))).scrollIntoView());
    status.textContent = "Block added. Edit its heading and content below.";
  }
  for (const [type, name, description] of [["accordion", "Accordion", "A heading with expandable rich text"], ["repeatable", "Repeatable text", "Add, reorder and remove labeled entries"], ["embed", "Iframe embed", "Embed a video, form or external page"]]) {
    const choice = document.createElement("button");
    choice.type = "button";
    choice.className = "rich-insert-choice";
    const title = document.createElement("strong");
    title.textContent = name;
    const hint = document.createElement("span");
    hint.textContent = description;
    choice.append(title, hint);
    choice.addEventListener("mousedown", (event) => event.preventDefault());
    choice.onclick = () => {
      closeMenu();
      if (type === "embed") openEmbedDialog(shell, null, (attrs) => insertBlock({ type: "embed", attrs }), () => editor.view.focus());
      else insertBlock(blockTemplate(type));
    };
    insertMenu.append(choice);
  }
  input.onchange = () => {
    if (input.files[0]) upload(input.files[0]);
    input.value = "";
  };
  async function upload(file) {
    if (uploading || destroyed) return;
    if (!file.type.startsWith("image/")) {
      status.textContent = "Choose an image file.";
      return;
    }
    if (file.size > 25 * 1024 * 1024) {
      status.textContent = "Images must be 25 MB or smaller.";
      return;
    }
    uploading = true;
    bookmark = editor.state.selection.getBookmark();
    status.textContent = "Uploading image\u2026";
    refresh();
    try {
      const body = new FormData();
      body.append("file", file);
      const response = await fetch(`/api/v1/collections/${collectionId}/assets`, { method: "POST", credentials: "include", body });
      if (!response.ok) {
        const error = await response.json().catch(() => ({}));
        throw Error(error.error || `Upload failed (${response.status})`);
      }
      const asset = await response.json();
      if (destroyed) return;
      if (bookmark) editor.view.dispatch(editor.state.tr.setSelection(bookmark.resolve(editor.state.doc)));
      const attrs = { src: `/api/v1/collections/${collectionId}/assets/${asset.id}/content`, assetId: asset.id };
      insertBlock({ type: "image", attrs });
      status.textContent = "Image added.";
    } catch (error) {
      if (!destroyed) status.textContent = error.message;
    } finally {
      uploading = false;
      bookmark = null;
      if (!destroyed) refresh();
    }
  }
  refresh();
  return { update(json2) {
    if (canonical(toPortable(editor.getJSON())) !== canonical(JSON.parse(json2))) editor.commands.setContent(toTiptap(JSON.parse(json2), collectionId), { emitUpdate: false });
  }, destroy() {
    destroyed = true;
    editor.destroy();
    shell.remove();
  } };
}
function download(name, bytes, type) {
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  const link = document.createElement("a"); link.href = url; link.download = name;
  document.body.appendChild(link); link.click(); link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
window.VulseEditor = { download, mount, update: (handle, json) => handle.update(json), destroy: (handle) => handle.destroy(), copy: (text) => navigator.clipboard.writeText(text) };
