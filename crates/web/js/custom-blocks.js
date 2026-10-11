import { Node } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";

export const blockTypes = ["embed", "accordion", "repeatable", "repeatableItem"];
const paragraph = () => ({ type: "paragraph" });
const entry = () => ({ type: "repeatableItem", attrs: { label: "" }, content: [paragraph()] });
const framePolicy = { sandbox: "allow-scripts allow-forms allow-presentation", referrerpolicy: "strict-origin-when-cross-origin", loading: "lazy", allow: "fullscreen", allowfullscreen: "true" };

export function embedUrl(value) {
  if (value.length > 2048 || /[\s\u0000-\u001f\u007f\\]/u.test(value) || !/^https?:\/\//i.test(value)) throw Error("Use a full HTTP or HTTPS URL, without spaces or credentials.");
  const url = new URL(value);
  if (!url.hostname || url.username || url.password) throw Error("Use a full HTTP or HTTPS URL, without credentials.");
  // Recognize share links, so users do not need to find a provider's embed URL.
  if (["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"].includes(url.hostname)) {
    const id = url.hostname === "youtu.be" ? url.pathname.slice(1) : url.pathname === "/watch" ? url.searchParams.get("v") : /^\/(?:embed|shorts)\/([^/]+)$/.exec(url.pathname)?.[1];
    if (id && /^[\w-]{11}$/.test(id)) {
      const embed = new URL(`https://www.youtube-nocookie.com/embed/${id}`);
      const time = url.searchParams.get("start") || url.searchParams.get("t");
      if (time && /^\d+$/.test(time)) embed.searchParams.set("start", time);
      return embed.href;
    }
  }
  if (["vimeo.com", "www.vimeo.com"].includes(url.hostname) && /^\/\d+$/.test(url.pathname)) return `https://player.vimeo.com/video${url.pathname}`;
  return url.href;
}

function el(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}
function button(label, action) {
  const control = el("button", "rich-block-button", label);
  control.type = "button";
  control.addEventListener("mousedown", (event) => event.preventDefault());
  control.onclick = action;
  return control;
}

export function openEmbedDialog(shell, initial, save, returnFocus) {
  const dialog = el("dialog", "rich-block-dialog");
  const form = el("form");
  const heading = el("h3", null, initial ? "Edit iframe embed" : "Add iframe embed");
  heading.id = `embed-dialog-${Math.random().toString(36).slice(2)}`;
  dialog.setAttribute("aria-labelledby", heading.id);
  form.append(heading, el("p", "muted", "Paste an embed URL or a YouTube / Vimeo share link. Some websites do not allow embedding."));
  function field(label, type, value) {
    const wrapper = el("label", "rich-dialog-field", label);
    const input = el("input");
    input.type = type;
    input.value = value;
    input.required = true;
    wrapper.append(input);
    form.append(wrapper);
    return input;
  }
  const url = field("Embed URL", "url", initial?.url || "");
  url.placeholder = "https://…";
  const title = field("Embed title", "text", initial?.title || "Embedded content");
  title.maxLength = 200;
  const height = field("Height (pixels)", "number", initial?.height || 360);
  height.min = 160;
  height.max = 1200;
  height.step = 1;
  const error = el("p", "error");
  error.setAttribute("role", "alert");
  const actions = el("div", "rich-block-actions");
  actions.append(button("Cancel", () => dialog.close()));
  const submit = button(initial ? "Update embed" : "Add embed", () => {});
  submit.type = "submit";
  actions.append(submit);
  form.append(error, actions);
  form.onsubmit = (event) => {
    event.preventDefault();
    try {
      if (!title.value.trim()) throw Error("Give the embed a title.");
      const attrs = { url: embedUrl(url.value.trim()), title: title.value.trim(), height: Number(height.value) };
      if (!Number.isInteger(attrs.height) || attrs.height < 160 || attrs.height > 1200) throw Error("Choose a height between 160 and 1200 pixels.");
      save(attrs);
      dialog.close();
    } catch (reason) { error.textContent = reason.message; }
  };
  dialog.append(form);
  shell.append(dialog);
  dialog.addEventListener("close", () => { dialog.remove(); returnFocus(); }, { once: true });
  dialog.showModal();
  url.focus();
}

function nodeView(kind, openEmbed) {
  return ({ editor, node: initial, getPos }) => {
    let node = initial;
    let expanded = !node.attrs.collapsed;
    const dom = el("section", `rich-custom-block rich-${kind}`);
    dom.dataset.vulseBlock = kind;
    const header = el("div", "rich-block-header");
    header.contentEditable = "false";
    const actions = el("div", "rich-block-actions");
    const contentDOM = kind === "embed" ? null : el("div", "rich-block-content");
    if (contentDOM) contentDOM.id = `block-content-${Math.random().toString(36).slice(2)}`;
    const pos = () => getPos();
    const update = (attrs) => {
      const position = pos();
      if (typeof position === "number") editor.view.dispatch(editor.state.tr.setNodeMarkup(position, undefined, { ...node.attrs, ...attrs }));
    };
    function focusContent(position = pos()) {
      if (typeof position !== "number") return;
      const selection = TextSelection.near(editor.state.doc.resolve(position + 1));
      editor.view.dispatch(editor.state.tr.setSelection(selection).scrollIntoView());
      editor.view.focus();
    }
    const label = el("span", "rich-block-kind", { accordion: "Accordion", repeatable: "Repeatable text", repeatableItem: "Entry", embed: "Iframe embed" }[kind]);
    header.append(label);
    let input;
    let toggle;
    let collapsed;
    let preview;
    let frame;
    if (kind === "embed") {
      preview = el("div", "rich-embed-preview");
      preview.contentEditable = "false";
      const caption = el("strong");
      const source = el("p", "muted");
      const load = button("Load preview", () => {
        if (frame) { frame.remove(); frame = null; load.textContent = "Load preview"; return; }
        frame = el("iframe");
        for (const [key, value] of Object.entries(framePolicy)) frame.setAttribute(key, value);
        frame.src = embedUrl(node.attrs.url);
        frame.title = node.attrs.title;
        frame.style.height = `${node.attrs.height}px`;
        preview.append(frame);
        load.textContent = "Hide preview";
      });
      actions.append(button("Edit embed", () => openEmbed(node.attrs, (attrs) => update(attrs))));
      preview.append(caption, source, load, el("p", "rich-block-hint", "If the preview is blocked by the provider, use its dedicated embed URL."));
      preview.refresh = () => {
        caption.textContent = node.attrs.title;
        source.textContent = node.attrs.url;
        if (frame) {
          const source = embedUrl(node.attrs.url);
          if (frame.src !== source) frame.src = source;
          frame.title = node.attrs.title;
          frame.style.height = `${node.attrs.height}px`;
        }
      };
    } else {
      input = el("input", "rich-block-title");
      input.type = "text";
      input.maxLength = 200;
      input.setAttribute("aria-label", kind === "repeatableItem" ? "Entry label" : kind === "accordion" ? "Accordion heading" : "Repeatable group title");
      input.placeholder = kind === "repeatableItem" ? "Optional entry label" : "Heading";
      input.oninput = () => update({ [kind === "repeatableItem" ? "label" : "title"]: input.value });
      input.onkeydown = (event) => { if (event.key === "Enter") { event.preventDefault(); expanded = true; sync(); focusContent(); } };
      header.append(input);
      if (kind === "accordion") {
        toggle = button("Collapse", () => { expanded = !expanded; sync(); });
        toggle.setAttribute("aria-controls", contentDOM.id);
        actions.append(toggle);
        const wrapper = el("label", "rich-block-checkbox");
        collapsed = el("input");
        collapsed.type = "checkbox";
        collapsed.onchange = () => update({ collapsed: collapsed.checked });
        wrapper.append(collapsed, document.createTextNode("Start collapsed"));
        header.append(wrapper);
      }
    }
    function move(direction) {
      const position = pos();
      if (typeof position !== "number") return;
      const resolved = editor.state.doc.resolve(position);
      const index = resolved.index();
      const parent = resolved.parent;
      const neighbor = parent.maybeChild(index + direction);
      if (!neighbor) return;
      const target = direction < 0 ? position - neighbor.nodeSize : position + neighbor.nodeSize;
      editor.view.dispatch(editor.state.tr.delete(position, position + node.nodeSize).insert(target, node).scrollIntoView());
    }
    const up = button("Move up", () => move(-1));
    const down = button("Move down", () => move(1));
    const remove = button(kind === "repeatableItem" ? "Remove entry" : "Remove block", () => {
      const position = pos();
      if (typeof position !== "number") return;
      editor.view.dispatch(editor.state.tr.delete(position, position + node.nodeSize));
      editor.view.focus();
    });
    remove.classList.add("rich-block-remove");
    actions.append(up, down, remove);
    header.append(actions);
    dom.append(header);
    if (contentDOM) dom.append(contentDOM);
    if (preview) dom.append(preview);
    let add;
    if (kind === "repeatable") {
      const footer = el("div", "rich-block-footer");
      footer.contentEditable = "false";
      add = button("Add entry", () => {
        const position = pos();
        if (typeof position !== "number" || node.childCount >= 100) return;
        const insert = position + node.nodeSize - 1;
        editor.commands.insertContentAt(insert, entry());
        focusContent(insert);
      });
      footer.append(add, el("span", "rich-block-hint", "Each entry supports rich text, images and blocks."));
      dom.append(footer);
    }
    function sync() {
      if (input) {
        const value = node.attrs[kind === "repeatableItem" ? "label" : "title"];
        if (input.value !== value) input.value = value;
      }
      if (toggle) {
        toggle.textContent = expanded ? "Collapse" : "Expand";
        toggle.setAttribute("aria-expanded", String(expanded));
        contentDOM.hidden = !expanded;
        collapsed.checked = node.attrs.collapsed;
      }
      if (preview) preview.refresh();
      const position = pos();
      if (typeof position === "number") {
        const resolved = editor.state.doc.resolve(position);
        up.disabled = resolved.index() === 0;
        down.disabled = resolved.index() === resolved.parent.childCount - 1;
        remove.disabled = kind === "repeatableItem" && resolved.parent.childCount === 1;
      }
      if (add) add.disabled = node.childCount >= 100;
    }
    editor.on("transaction", sync);
    sync();
    return {
      dom, ...(contentDOM ? { contentDOM } : {}),
      update(next) {
        if (next.type !== node.type) return false;
        if (next.attrs.collapsed !== node.attrs.collapsed) expanded = !next.attrs.collapsed;
        node = next;
        sync();
        return true;
      },
      stopEvent: (event) => !contentDOM || !contentDOM.contains(event.target),
      ignoreMutation: (mutation) => mutation.type !== "selection" && (!contentDOM || (mutation.type === "attributes" && mutation.target === contentDOM) || (mutation.target !== contentDOM && !contentDOM.contains(mutation.target))),
      destroy: () => editor.off("transaction", sync)
    };
  };
}

export function customBlockExtensions(openEmbed) {
  const Embed = Node.create({
    name: "embed", group: "block", atom: true, draggable: true,
    addAttributes: () => ({ url: { default: "" }, title: { default: "Embedded content" }, height: { default: 360 } }),
    parseHTML: () => [{ tag: "iframe[src]", getAttrs(element) {
      try { return { url: embedUrl(element.getAttribute("src")), title: element.getAttribute("title")?.trim().slice(0, 200) || "Embedded content", height: Math.round(Math.max(160, Math.min(1200, Number(element.getAttribute("height")) || 360))) }; } catch { return false; }
    } }],
    renderHTML: ({ node }) => ["iframe", { ...framePolicy, src: embedUrl(node.attrs.url), title: node.attrs.title, height: node.attrs.height }],
    addNodeView: () => nodeView("embed", openEmbed)
  });
  const container = (name, content, attrs) => Node.create({
    name, ...(name !== "repeatableItem" ? { group: "block" } : {}), content, defining: true, isolating: true,
    addAttributes: () => Object.fromEntries(Object.entries(attrs).map(([key, value]) => [key, { default: value, rendered: false }])),
    parseHTML: () => [{ tag: `section[data-vulse-block="${name}"]`, contentElement: ".rich-serialized-content", getAttrs: (element) => Object.fromEntries(Object.entries(attrs).map(([key, fallback]) => [key, key === "collapsed" ? element.getAttribute(`data-${key}`) === "true" : (element.getAttribute(`data-${key}`) || fallback).slice(0, 200)])) }],
    renderHTML: ({ node }) => ["section", { "data-vulse-block": name, ...Object.fromEntries(Object.keys(attrs).map((key) => [`data-${key}`, String(node.attrs[key])])) }, ["div", { class: "rich-serialized-content" }, 0]],
    addNodeView: () => nodeView(name, openEmbed)
  });
  return [Embed, container("accordion", "block+", { title: "Accordion", collapsed: false }), container("repeatable", "repeatableItem+", { title: "Repeatable text" }), container("repeatableItem", "block+", { label: "" })];
}

export function blockTemplate(type) {
  if (type === "accordion") return { type, attrs: { title: "Accordion", collapsed: false }, content: [paragraph()] };
  if (type === "repeatable") return { type, attrs: { title: "Repeatable text" }, content: [entry()] };
  throw Error("Unknown block type");
}
