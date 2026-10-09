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
  const started = await send({ kind: "start", request: p({ width: 420 * 1024, height: 300 * 1024, max_pages: 1000, viewport_start: 0, viewport_end: 1 }) });
  assert.equal(started.kind, "started");
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
  assert.equal((await send({ kind: "close" })).kind, "ack");
  console.log("Worker smoke: edit effects, created IDs, split selection, emphasis/decoration/colour, authored marks and image results passed");
})().catch(error => { console.error(error); process.exitCode = 1; });
