// Run the compiled reference worker against the actual Node WASM bindings.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const wasm = require(path.resolve(process.argv[2], "reprise_wasm.js"));
const pending = new Map();
const scope = { postMessage(response) { pending.get(response.data.id)(response.data); } };
vm.runInNewContext(fs.readFileSync(process.argv[3], "utf8"), {
  exports: {}, self: scope,
  require(name) {
    assert.equal(name, "./reprise_wasm.js");
    return { ...wasm, default: async () => {} };
  }
});
const p = data => ({ version: 1, data });
let next = 0;
function send(data) {
  const id = String(++next);
  return new Promise(resolve => {
    pending.set(id, response => { pending.delete(id); resolve(response); });
    scope.onmessage({ data: p({ ...data, id }) });
  });
}
(async () => {
  assert.equal((await send({ kind: "create", request: p({ document_id: "00112233445566778899aabbccddeeff", peer_id: "51" }) })).kind, "state");
  const inserted = await send({ kind: "edit", request: p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "abcdef", style: { families: null, size: null, line_height: null } }] }) });
  assert.equal(inserted.kind, "edited");
  const node = inserted.applied.data.blocks[0];
  assert.equal(inserted.state.data.blocks[0].id, node);
  const formatted = await send({ kind: "edit", request: p({ commands: [{ kind: "format-text", node, start: 0, end: 6, style: { weight: 700, slant: "oblique", decoration: { underline: true, strike: true }, color: [20, 40, 60, 0] } }] }) });
  assert.equal(formatted.kind, "edited");
  const styled = formatted.state.data.blocks[0].formatting[0].style;
  assert.equal(styled.weight, 700); assert.equal(styled.slant, "oblique");
  assert.equal(styled.decoration.underline, true); assert.equal(styled.decoration.strike, true);
  assert.deepEqual(Array.from(styled.color), [20, 40, 60, 0]);
  const split = await send({ kind: "edit", request: p({ commands: [{ kind: "split-block", node, at: 3 }] }) });
  assert.equal(split.kind, "edited");
  const effect = split.applied.data.effects.find(e => e.kind === "split");
  assert.equal(effect.node, node);
  assert.equal(effect.at, 3);
  assert.equal(split.state.data.blocks.find(b => b.id === effect.new).text, "def");
  assert.equal(split.state.data.blocks.find(b => b.id === effect.new).formatting[0].style.weight, 700);
  // A UI transforms a caret at offset 5 over the split, then anchors it.
  const caret = { node: effect.new, offset: 5 - effect.at, affinity: "downstream" };
  const selection = await send({ kind: "selection", request: p({ anchor: caret, focus: caret }) });
  assert.equal(selection.kind, "selection");
  assert.equal(selection.selection.data.focus.node, effect.new);
  assert.equal(selection.selection.data.focus.offset, 2);
  const image = await send({ kind: "image", request: p({ at: null, asset: "0".repeat(64), alt: "image", width: 1024, height: 1024, style: { families: null, size: null, line_height: null } }) });
  assert.equal(image.kind, "edited");
  assert.ok(image.state.data.blocks.some(b => b.id === image.applied.data.blocks[0]));
  await send({ kind: "create", request: p({ document_id: "10112233445566778899aabbccddeeff", peer_id: "52" }) });
  const verse = await send({ kind: "edit", request: p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "ab", style: {} }] }) });
  const verseNode = verse.applied.data.blocks[0];
  const marksEdit = await send({ kind: "edit", request: p({ commands: [
    { kind: "insert-tab", node: verseNode, at: 1 },
    { kind: "insert-line-break", node: verseNode, at: 3 },
    { kind: "set-tab-stops", node: verseNode, tabs: { interval: 36 * 1024, stops: [{ position: null, alignment: "end", leader: null }] } },
    { kind: "set-alignment", node: verseNode, at: 4, alignment: "centre" },
    { kind: "add-anchor", node: verseNode, at: 4, edge: "start", target: verseNode, target_at: 1, target_edge: "gap-end" }
  ] }) });
  assert.equal(marksEdit.kind, "edited");
  assert.equal(marksEdit.state.data.blocks[0].text, "a\tb\n");
  const options = p({ width: 420 * 1024, height: 300 * 1024, max_pages: 1000, viewport_start: 0, viewport_end: 1 });
  const obsolete = await send({ kind: "start", request: options });
  assert.equal(obsolete.kind, "started");
  const configured = await send({ kind: "font-settings", request: p({ generics: { monospace: "host-mono" }, fallbacks: ["host-universal"], synthetic_oblique: true }) });
  assert.equal(configured.kind, "ack");
  assert.equal((await send({ kind: "step", job: obsolete.job, budget: 1 })).kind, "error");
  const started = await send({ kind: "start", request: options });
  assert.equal(started.kind, "started");
  const refused = await send({ kind: "font-settings", request: p({ generics: {}, fallbacks: Array(65).fill("host-universal"), synthetic_oblique: false }) });
  assert.equal(refused.kind, "error");
  assert.equal(refused.error.data.code, "font.config");
  let complete = false;
  for (let i = 0; i < 1000; i++) {
    const step = await send({ kind: "step", job: started.job, budget: 1000 });
    assert.equal(step.kind, "layout");
    if (step.progress.data.complete) { complete = true; break; }
  }
  assert.ok(complete);
  const marksResponse = await send({ kind: "marks", page: 0 });
  assert.equal(marksResponse.kind, "marks");
  assert.equal(marksResponse.page.data.page, 0);
  assert.deepEqual(Array.from(new Set(marksResponse.page.data.marks.map(m => m.kind))).sort(), ["alignment", "anchor", "gap", "line-break", "paragraph-end"]);
  assert.ok(marksResponse.page.data.marks.some(m => m.kind === "anchor" && m.applied && m.state === "valid"));
  assert.equal((await send({ kind: "marks", page: 99 })).kind, "error");
  // Plumbing: batch, atomic replace, workspace font, diagnostics, import as a copy.
  const plain = { families: null, size: null, line_height: null };
  await send({ kind: "create", request: p({ document_id: "20112233445566778899aabbccddeeff", peer_id: "53" }) });
  const base = await send({ kind: "edit", request: p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "hello world", style: plain }] }) });
  const baseNode = base.applied.data.blocks[0];
  const failing = await send({ kind: "batch", request: p({ commands: [
    { kind: "insert-text", node: baseNode, at: 0, text: ">" },
    { kind: "insert-block", parent: null, index: 1, block_kind: "paragraph", text: "b", style: plain },
    { kind: "delete-text", node: baseNode, start: 90, end: 99 }], expected_revision: null, selection: null }) });
  assert.equal(failing.kind, "error"); assert.equal(failing.error.data.command, 2);
  const batched = await send({ kind: "batch", request: p({ commands: [{ kind: "insert-text", node: baseNode, at: 0, text: ">> " }], expected_revision: base.state.data.revision, selection: null }) });
  assert.equal(batched.kind, "batched"); assert.equal(batched.state.data.blocks[0].text, ">> hello world");
  const anchored = await send({ kind: "selection", request: p({ anchor: { node: baseNode, offset: 9, affinity: "downstream" }, focus: { node: baseNode, offset: 14, affinity: "downstream" } }) });
  const replaced = await send({ kind: "replace-text", request: p({ selection: anchored.stable.data, text: "there", html: false }) });
  assert.equal(replaced.kind, "replaced"); assert.equal(replaced.state.data.blocks[0].text, ">> hello there");
  const diagnostics = await send({ kind: "diagnostics", request: p({ code_prefix: "relation.", min_severity: null, subject: null, limit: 5 }) });
  assert.equal(diagnostics.kind, "diagnostics"); assert.equal(diagnostics.list.data.truncated, false);
  const fontBytes = new Uint8Array(fs.readFileSync(path.join(__dirname, "../../../fixtures/fonts/AtkinsonHyperlegibleNext-Regular.ttf")));
  const face = await send({ kind: "workspace-font", request: p({ family: "Universal", weight: 400, style: "normal", stretch: 1000, face_index: 0 }), bytes: fontBytes });
  assert.equal(face.kind, "face"); assert.equal(face.face.data.family, "Universal");
  const saved = await send({ kind: "save" });
  const copied = await send({ kind: "import-copy", request: p({ document_id: "30112233445566778899aabbccddeeff", peer_id: "54" }), bytes: saved.content.data.bytes });
  assert.equal(copied.kind, "state"); assert.equal(copied.state.data.document_id, "30112233445566778899aabbccddeeff");
  assert.equal(copied.state.data.blocks[0].text, ">> hello there");
  const copiedNode = copied.state.data.blocks[0].id;
  const semantic = await send({ kind: "batch", request: p({ commands: [
    { kind: "set-role", node: copiedNode, role: "heading-3" },
    { kind: "create-list", definition: { levels: [{ level: 0, format: "decimal", start: "4" }] } },
    { kind: "set-list-item", node: copiedNode, list: { kind: "new", index: 0 }, level: 0 }
  ], expected_revision: copied.state.data.revision, selection: null }) });
  assert.equal(semantic.kind, "batched");
  assert.equal(semantic.state.data.blocks[0].list.marker, "4.");
  assert.equal(semantic.state.data.blocks[0].role, "heading-3");
  const outline = await send({ kind: "outline" });
  assert.equal(outline.kind, "outline"); assert.equal(outline.headings.data[0].level, 3);
  assert.equal(outline.headings.data[0].page, null);
  console.log("roles/lists worker smoke passed");
  // Annotations: commands through the edit kind, queries through their own.
  await send({ kind: "create", request: p({ document_id: "40112233445566778899aabbccddeeff", peer_id: "55" }) });
  const linked = await send({ kind: "edit", request: p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "read the manual now", style: plain }] }) });
  const linkedNode = linked.applied.data.blocks[0];
  const withLink = await send({ kind: "edit", request: p({ commands: [
    { kind: "add-annotation", node: linkedNode, start: 9, end: 15, annotation: { kind: "link", target: { type: "url", url: "https://manual.example" } } },
    { kind: "add-annotation", node: linkedNode, start: 0, end: 4, annotation: { kind: "comment", thread: "t-9" } }] }) });
  assert.equal(withLink.kind, "edited"); assert.equal(withLink.applied.data.annotations.length, 2);
  assert.equal(withLink.state.data.annotations.length, 2);
  const [linkId, noteId] = withLink.applied.data.annotations;
  const one = await send({ kind: "annotation", request: p(linkId) });
  assert.equal(one.kind, "annotation"); assert.equal(one.annotation.data.payload.target.url, "https://manual.example");
  const at = await send({ kind: "annotations-at", request: p({ node: linkedNode, at: 10 }) });
  assert.equal(at.kind, "annotations"); assert.deepEqual(at.list.data.annotations.map(x => x.id), [linkId]);
  assert.deepEqual((await send({ kind: "annotations-in", request: p({ node: linkedNode, start: 0, end: 19 }) })).list.data.annotations.map(x => x.id), [noteId, linkId]);
  assert.deepEqual((await send({ kind: "annotations-of-kind", request: p("comment") })).list.data.annotations.map(x => x.id), [noteId]);
  assert.equal((await send({ kind: "annotation-geometry", request: p(linkId) })).kind, "error");
  const linkedJob = await send({ kind: "start", request: p({ width: 420 * 1024, height: 300 * 1024, max_pages: 100, viewport_start: 0, viewport_end: 1 }) });
  for (let i = 0; i < 1000; i++) {
    const step = await send({ kind: "step", job: linkedJob.job, budget: 1000 });
    if (step.progress.data.complete) break;
  }
  const geometry = await send({ kind: "annotation-geometry", request: p(linkId) });
  assert.equal(geometry.kind, "annotation-geometry"); assert.ok(geometry.geometry.data.rects.length >= 1);
  const links = await send({ kind: "page-links", page: 0 });
  assert.equal(links.kind, "page-links"); assert.deepEqual(links.links.data.links.map(x => x.id), [linkId]);
  // Inline objects: commands through the edit kind, rectangles and targets through their own.
  const objectTarget = await send({ kind: "field-target", request: p({ node: linkedNode, at: 0 }) });
  assert.equal(objectTarget.kind, "field-target"); assert.ok(objectTarget.target.data.startsWith(linkedNode));
  const withObject = await send({ kind: "edit", request: p({ commands: [
    { kind: "insert-inline", node: linkedNode, at: 19, object: { kind: "field", align: "baseline", params: { field: "page-number" } } }] }) });
  assert.equal(withObject.kind, "edited"); assert.equal(withObject.state.data.blocks[0].inline.length, 1);
  const objectJob = await send({ kind: "start", request: p({ width: 420 * 1024, height: 300 * 1024, max_pages: 100, viewport_start: 0, viewport_end: 1 }) });
  for (let i = 0; i < 1000; i++) {
    const step = await send({ kind: "step", job: objectJob.job, budget: 1000 });
    if (step.progress.data.complete) break;
  }
  const atom = await send({ kind: "atom-rect", request: p({ node: linkedNode, at: 19 }) });
  assert.equal(atom.kind, "atom-rect"); assert.equal(atom.rect.data.page, 0); assert.ok(atom.rect.data.rect.width > 0);
  assert.equal((await send({ kind: "atom-rect", request: p({ node: linkedNode, at: 3 }) })).kind, "error");
  assert.equal((await send({ kind: "close" })).kind, "ack");
  console.log("Worker smoke: edit effects, created IDs, split selection, emphasis/decoration/colour, authored marks, host font configuration/job invalidation and image results passed");
})().catch(error => { console.error(error); process.exitCode = 1; });
