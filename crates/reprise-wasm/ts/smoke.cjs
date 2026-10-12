// Run after generating the Node package: node ts/smoke.cjs <pkg-dir>
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const wasm = require(path.resolve(process.argv[2], "reprise_wasm.js"));
const p = data => ({ version: 1, data });
const ws = new wasm.Workspace();
const doc = ws.create(p({ document_id: "00112233445566778899aabbccddeeff", peer_id: "1" }));
const inserted = doc.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "office ffi אבג", style: { families: ["Source Serif Pro", "serif"], size: null, line_height: null } }] }));
const node = inserted.data.blocks[0];
const options = p({ width: 420 * 1024, height: 300 * 1024, max_pages: 1000, viewport_start: 0, viewport_end: 1 });
function finish(doc) { const job = doc.start_layout(options).data; for (let i = 0; i < 10_000; i++) { const progress = doc.step(job, 1); if (progress.data.complete) { doc.release_job(job); return; } } throw new Error("explicit smoke job bound exceeded"); }
finish(doc);
assert.equal(doc.display_json(0).data, fs.readFileSync(path.join(__dirname, "../../reprise/tests/display.json"), "utf8").trimEnd());
assert.throws(() => doc.apply({ version: 99, data: {} }), e => e.version === 1 && e.data.code === "bindings.version");
assert.throws(() => doc.apply(p({ commands: [{ kind: "insert-text", node: "invalid", at: 0, text: "bad" }] })), e => e.data.code === "bindings.id");
const cyc = p({ commands: [] }); cyc.data.extra = cyc;
assert.throws(() => doc.apply(cyc), e => e.data.code === "bindings.invalid");
const caret = { node, offset: 0, affinity: "downstream" };
assert.deepEqual(doc.selection_rects(p({ anchor: caret, focus: caret })).data, []);
const selection = p({ anchor: caret, focus: { node, offset: Buffer.byteLength("office ffi אבג"), affinity: "upstream" } });
const clip = doc.copy(selection).data.bytes;
assert.ok(clip instanceof Uint8Array);
doc.paste(p({ at: null }), clip); doc.undo(); finish(doc);
const saved = doc.save().data.bytes;
assert.ok(saved instanceof Uint8Array);
const second = ws.open(p({ peer_id: "2" }), saved); finish(second);
assert.equal(second.display_json(0).data, doc.display_json(0).data);
const before = doc.start_layout(options).data;
doc.apply(p({ commands: [{ kind: "insert-text", node, at: 0, text: "A" }] }));
assert.throws(() => doc.step(before, 1), e => e.data.code === "bindings.stale");doc.release_job(before);
second.import_updates(doc.export_updates());
finish(doc);finish(second);assert.equal(doc.display_json(0).data, second.display_json(0).data);
assert.ok(doc.export(p("pdf")).data.content.bytes instanceof Uint8Array);
assert.ok(doc.png(0, 1000).data.bytes instanceof Uint8Array);
assert.ok(doc.awareness(new Uint8Array([0, 255])).data.content.bytes instanceof Uint8Array);
assert.throws(() => doc.awareness("wrong byte type"), e => e.data.code === "bindings.invalid");
let reads = 0;
const getterRequest = p({ get commands() { reads++; return []; } });
doc.apply(getterRequest); assert.equal(reads, 1);
const throwing = p({ get commands() { throw new Error("getter"); } });
assert.throws(() => doc.apply(throwing), e => e.data.code === "bindings.invalid");
const deep = p({ commands: [] }); let deepAt = deep.data; for (let i = 0; i < 66; i++) deepAt = deepAt.extra = {};
assert.throws(() => doc.apply(deep), e => e.data.code === "bindings.limit");
const moduleBytes = new Uint8Array(fs.readFileSync(path.join(__dirname, "../../plugin/test-plugins/demo.wasm")));
const crypto = require("node:crypto");
const plugin = doc.load_plugin(p({ name: "demo", plugin_version: "1", sha256: crypto.createHash("sha256").update(moduleBytes).digest("hex"), phase: "layout", imports: [], grants: [], functions: [{ name: "demo-double", operation: 1, params: ["length"], variadic: null, returns: "length" }], fuel: 100000, memory_pages: 16, table_elements: 1024, buffer_bytes: 65536 }), moduleBytes).data;
doc.install_plugin(plugin, p({ kind: "functions" }));
doc.install_plugin(plugin, p({ kind: "geometry", operation: 2 }));
doc.apply(p({ commands: [{ kind: "set-style", node, style: { families: null, size: "demo-double(6pt)", line_height: null } }] }));
finish(doc);
assert.ok(!doc.diagnostics().data.some(note => note.code === "style.function-failed" || note.code === "plugin.trap"));
// Default style needs neither a family chain nor host font registration.
const defaultDoc = ws.create(p({ document_id: "10112233445566778899aabbccddeeff", peer_id: "21" }));
const defaultNode = defaultDoc.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "office ffi café אבג مرحبا é", style: { families: null, size: null, line_height: null } }] })).data.blocks[0];
finish(defaultDoc);
assert.ok(defaultDoc.diagnostics().data.every(note => note.severity === "info"));
assert.ok(defaultDoc.copy(p({ anchor: { node: defaultNode, offset: 0, affinity: "downstream" }, focus: { node: defaultNode, offset: 6, affinity: "upstream" } })).data.bytes instanceof Uint8Array);
const imageDoc = ws.create(p({ document_id: "20112233445566778899aabbccddeeff", peer_id: "22" }));
const red = new Uint8Array(fs.readFileSync(path.join(__dirname, "../../../fixtures/images/red-1x1.png")));
const asset = imageDoc.register_asset(p({ id: "red", kind: "image" }), red).data;
const image = { at: null, asset, alt: "A red café image", width: 20 * 1024, height: 20 * 1024, style: { families: null, size: null, line_height: null } };
const imageNode = imageDoc.apply(p({ commands: [{ kind: "insert-image", image }] })).data.blocks[0];
finish(imageDoc);
const drawing = imageDoc.display_json(0).data;
assert.equal(drawing, fs.readFileSync(path.join(__dirname, "../../reprise/tests/images.json"), "utf8").trimEnd());
assert.ok(drawing.includes('"type":"image"') && drawing.includes(asset));
assert.ok(imageDoc.diagnostics().data.every(note => note.severity === "info"));
const vector = imageDoc.svg(0).data;
assert.ok(vector.includes("data:image/png;base64,"));
const raster = imageDoc.png(0, 1000).data.bytes;
const pdfImage = imageDoc.export(p("pdf"));
assert.ok(pdfImage.data.content.bytes instanceof Uint8Array);
assert.equal(pdfImage.data.losses.find(loss => loss.code === "export.assets").disposition, "preserved");
const imageCopy = imageDoc.copy(p({ anchor: { node: imageNode, offset: 0, affinity: "downstream" }, focus: { node: imageNode, offset: Buffer.byteLength(image.alt), affinity: "upstream" } })).data.bytes;
const imageTarget = ws.create(p({ document_id: "30112233445566778899aabbccddeeff", peer_id: "23" }));
imageTarget.paste(p({ at: null }), imageCopy); finish(imageTarget);
assert.equal(imageTarget.svg(0).data, vector);
assert.deepEqual(imageTarget.png(0, 1000).data.bytes, raster);
imageTarget.undo(); assert.equal(imageTarget.state().data.blocks.length, 0); imageTarget.redo(); finish(imageTarget);
const imageOpen = ws.open(p({ peer_id: "24" }), imageTarget.save().data.bytes); finish(imageOpen);
assert.equal(imageOpen.display_json(0).data, drawing);
assert.equal(imageOpen.svg(0).data, vector);
assert.deepEqual(imageOpen.png(0, 1000).data.bytes, raster);
const missingDoc = ws.create(p({ document_id: "40112233445566778899aabbccddeeff", peer_id: "25" }));
missingDoc.insert_image(p(image)); finish(missingDoc);
assert.notDeepEqual(missingDoc.png(0, 1000).data.bytes, raster);
missingDoc.free(); imageOpen.free(); imageTarget.free(); imageDoc.free(); defaultDoc.free();
// Multiplayer methods use the same typed boundary and byte representation.
const remote = ws.create(p({ document_id: "00112233445566778899aabbccddeeff", peer_id: "31" }));
const full = doc.sync_export(p({ since: null }));
assert.deepEqual(full.data.features, ["snapshot-json"]);
remote.sync_import(full);
const multiplayerSelection = { anchor: { node, offset: 2, affinity: "downstream" }, focus: { node, offset: 4, affinity: "upstream" } };
const stable = remote.set_selection(p(multiplayerSelection));
assert.deepEqual(remote.resolve_selection(stable).data, multiplayerSelection);
const vv = remote.sync_info().data.vector;
doc.apply(p({ commands: [{ kind: "insert-text", node, at: 0, text: "remote " }] }));
const packet = doc.sync_export(p({ since: vv }));
assert.ok(packet.data.content.bytes instanceof Uint8Array);
const report = remote.sync_import(packet).data;
assert.ok(report.changed && report.changes.blocks.includes(node));
assert.equal(report.selection.focus.offset, 11);
assert.equal(remote.sync_import(packet).data.changed, false);
finish(remote);
const awareness = doc.presence(p({ selection: { anchor: { node, offset: 0, affinity: "downstream" }, focus: { node, offset: 3, affinity: "upstream" } }, meta: { name: "Remote" } }));
const view = remote.resolve_presence(awareness).data;
assert.equal(view.meta.name, "Remote");
assert.ok(view.caret && view.rects.length);
const incoming = remote.state().data.blocks.find(b => b.id === node).text;
remote.apply(p({ commands: [{ kind: "insert-text", node, at: 0, text: "local " }] }));
assert.equal(remote.undo_report().data.changed, true);
assert.equal(remote.state().data.blocks.find(b => b.id === node).text, incoming);
assert.equal(remote.redo_report().data.changed, true);
remote.set_selection(p(null)); assert.equal(remote.local_selection().data, null);
remote.free();
// Run the native packet-golden script through actual WASM bindings.
const parity = ws.create(p({ document_id: "00112233445566778899aabbccddeeff", peer_id: "1" }));
const parityNode = parity.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "parity", style: { families: null, size: null, line_height: null } }] })).data.blocks[0];
parity.apply(p({ commands: [{ kind: "insert-text", node: parityNode, at: 6, text: " אב" }] }));
const parityPacket = parity.sync_export(p({ since: [] }));
assert.equal(Buffer.from(parityPacket.data.content.bytes).toString("hex"), fs.readFileSync(path.join(__dirname, "../../reprise/tests/sync_delta.hex"), "utf8").trim());
// Character formatting crosses the boundary as runs; glyph outlines draw what layout placed.
parity.apply(p({ commands: [{ kind: "format-text", node: parityNode, start: 0, end: 3, style: { families: null, size: 20 * 1024, language: null, features: null, reset: false, weight: 700, slant: "italic", decoration: { underline: true, strike: true }, color: [1, 2, 3, 0] } }] }));
const runs = parity.state().data.blocks.find(b => b.id === parityNode).formatting;
assert.deepEqual(runs.map(r => [r.start, r.end]), [[0, 3], [3, 11]]);
assert.equal(runs[0].style.size, 20 * 1024);
assert.equal(runs[0].style.weight, 700);
assert.equal(runs[0].style.slant, "italic");
assert.deepEqual(runs[0].style.decoration, { underline: true, strike: true });
assert.deepEqual(runs[0].style.color, [1, 2, 3, 0]);
assert.throws(() => parity.apply(p({ commands: [{ kind: "format-text", node: parityNode, start: 0, end: 1, style: { weight: 1001 } }] })));
finish(parity);
const glyphRuns = [];
(function collect(items) { for (const item of items) { if (item.type === "glyphs") glyphRuns.push(item); else if (item.type === "group") collect(item.items); } })(parity.display_page(0).data.display.items);
assert.ok(glyphRuns.length > 0);
assert.ok(glyphRuns.some(run => run.color.join(",") === "1,2,3,0"));
const decoratedDisplay = parity.display_json(0).data;
assert.ok(decoratedDisplay.includes('"type":"path"'));
assert.ok(parity.diagnostics().data.some(d => d.code === "font.nearest" && d.severity === "warning"));
const emphasisSave = parity.save().data.bytes;
const emphasisReopen = ws.open(p({ peer_id: "3" }), emphasisSave);
assert.deepEqual(emphasisReopen.state().data.blocks[0].formatting, parity.state().data.blocks[0].formatting);
emphasisReopen.free();
for (const run of glyphRuns) {
  const ids = run.glyphs.map(g => g.id);
  const outlines = parity.glyph_outlines(p({ face: run.face, glyphs: ids })).data;
  assert.ok(outlines.units_per_em > 0);
  assert.deepEqual(outlines.glyphs.map(g => g.id), ids);
}
assert.throws(() => parity.glyph_outlines(p({ face: { family: "Nope", hash: "00".repeat(32) }, glyphs: [1] })), e => e.data.code === "bindings.missing-font");
// Host configuration is separate from document state and invalidates old jobs.
parity.apply(p({ commands: [{ kind: "insert-text", node: parityNode, at: 1, text: "é" }] }));
const fontRevision = parity.state().data.revision;
const oldFontJob = parity.start_layout(options).data;
const fontDir = path.join(__dirname, "../../../fixtures/fonts");
parity.declare_font(p({ family: "Paper Mono", weight: 400, style: "normal", stretch: 1000, face_index: 0 }), new Uint8Array(fs.readFileSync(path.join(fontDir, "PaperMono-TestSubset.ttf"))));
parity.declare_font(p({ family: "Universal", weight: 400, style: "normal", stretch: 1000, face_index: 0 }), new Uint8Array(fs.readFileSync(path.join(fontDir, "AtkinsonHyperlegibleNext-Regular.ttf"))));
parity.set_font_settings(p({ generics: { serif: "Paper Mono" }, fallbacks: ["Universal"], synthetic_oblique: true }));
assert.deepEqual(parity.state().data.revision, fontRevision);
assert.throws(() => parity.step(oldFontJob, 1)); parity.release_job(oldFontJob);
finish(parity);
assert.ok(parity.display_json(0).data.includes('"synthetic_oblique":true'));
assert.ok(parity.display_json(0).data.includes('"family":"Universal"'));
assert.ok(parity.diagnostics().data.some(d => d.code === "font.synthetic" && d.severity === "info"));
assert.ok(parity.diagnostics().data.some(d => d.code === "font.fallback"));
const styledRuns = [];
(function collect(items) { for (const item of items) { if (item.type === "glyphs" && item.synthetic_oblique) styledRuns.push(item); else if (item.type === "group") collect(item.items); } })(parity.display_page(0).data.display.items);
for (const run of styledRuns) {
  const glyphs = run.glyphs.map(g => g.id);
  const plain = parity.glyph_outlines(p({ face: run.face, glyphs })).data;
  const styled = parity.glyph_outlines_styled(p({ face: run.face, glyphs, synthetic_oblique: true })).data;
  assert.equal(styled.units_per_em, plain.units_per_em);
  assert.notDeepEqual(styled.glyphs, plain.glyphs);
  assert.ok(styled.glyphs.filter(g => g.path).every(g => g.ink_bounds && g.ink_bounds.x_min <= g.ink_bounds.x_max));
}
assert.throws(() => parity.set_font_settings(p({ generics: {}, fallbacks: Array(65).fill("bad"), synthetic_oblique: true })), e => e.data.code === "font.config");
assert.throws(() => parity.set_font_settings(p({ generics: { unknown: "bad" }, fallbacks: [], synthetic_oblique: false })));
// Authored paper edits retain host font settings; resetting fonts retains paper.
parity.set_page_setup(p({ width: 240 * 1024, height: 320 * 1024, right: 36 * 1024 }));
finish(parity);
assert.equal(parity.display_page(0).data.display.width, 240 * 1024);
assert.ok(parity.display_json(0).data.includes('"synthetic_oblique":true'));
assert.ok(parity.display_json(0).data.includes('"family":"Universal"'));
parity.set_font_settings(p({ generics: {}, fallbacks: [], synthetic_oblique: false }));
finish(parity); assert.ok(!parity.display_json(0).data.includes('"synthetic_oblique"'));
assert.equal(parity.state().data.page_setup.width, 240 * 1024);
parity.free();
// Authored positions survive the real WASM command, query and package paths.
const marksDoc = ws.create(p({ document_id: "50112233445566778899aabbccddeeff", peer_id: "61" }));
assert.throws(() => marksDoc.marks(0), e => e.data.code === "bindings.layout-required");
const marksNode = marksDoc.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "ab", style: {} }] })).data.blocks[0];
marksDoc.apply(p({ commands: [
  { kind: "insert-tab", node: marksNode, at: 1 },
  { kind: "insert-line-break", node: marksNode, at: 3 },
  { kind: "set-tab-stops", node: marksNode, tabs: { interval: 36 * 1024, stops: [{ position: null, alignment: "end", leader: "." }] } },
  { kind: "set-alignment", node: marksNode, at: null, alignment: "start" },
  { kind: "set-alignment", node: marksNode, at: 4, alignment: "centre" }
] }));
const pin = marksDoc.apply(p({ commands: [{ kind: "add-anchor", node: marksNode, at: 4, edge: "start", target: marksNode, target_at: 1, target_edge: "gap-end" }] })).data.relations[0];
finish(marksDoc);
const marksDisplay = marksDoc.display_json(0).data;
const marks = marksDoc.marks(0).data;
assert.deepEqual([...new Set(marks.marks.map(m => m.kind))].sort(), ["alignment", "anchor", "gap", "line-break", "paragraph-end"]);
assert.ok(marks.marks.some(m => m.relation === pin && m.state === "valid" && m.applied && m.from.x === m.to.x));
assert.equal(marksDoc.display_json(0).data, marksDisplay);
assert.throws(() => marksDoc.marks(99), e => e.data.code === "bindings.invalid");
const marksOpen = ws.open(p({ peer_id: "62" }), marksDoc.save().data.bytes); finish(marksOpen);
assert.deepEqual(marksOpen.marks(0).data.marks, marks.marks);
assert.equal(marksOpen.display_json(0).data, marksDisplay);
assert.ok(Buffer.from(marksDoc.export(p("html")).data.content.bytes).toString("utf8").includes("<br>"));
marksDoc.apply(p({ commands: [{ kind: "remove-anchor", id: pin }] })); finish(marksDoc);
assert.ok(marksDoc.marks(0).data.marks.every(m => m.relation !== pin));
marksDoc.undo(); finish(marksDoc);
assert.ok(marksDoc.marks(0).data.marks.some(m => m.relation === pin && m.applied));
marksOpen.free(); marksDoc.free();
second.free();doc.free();
console.log("WASM smoke: native JSON parity, editor loop, sync, defaults, images, formatting, emphasis/decoration/colour, host fonts, synthetic glyph outlines and ink bounds, authored marks, bytes, and hostile objects passed");

// Page setup is readable before layout, property-merged, undoable and packaged.
const paper = ws.create(p({ document_id: "99112233445566778899aabbccddeeff", peer_id: "90" }));
const paperPeer = ws.create(p({ document_id: "99112233445566778899aabbccddeeff", peer_id: "91" }));
assert.equal(paper.state().data.page_setup.width, 420 * 1024);
assert.equal(paper.state().data.page_setup.patched, false);
paperPeer.sync_import(paper.sync_export(p({ since: null })));
paper.set_page_setup(p({ width: 609562, height: 862095 }));
paperPeer.apply(p({ commands: [{ kind: "set-page-setup", setup: { top: 36864, right: 49152, bottom: 55296, left: 73728 } }] }));
const paperUpdate = paper.sync_export(p({ since: null }));
const marginUpdate = paperPeer.sync_export(p({ since: null }));
paper.sync_import(marginUpdate); paperPeer.sync_import(paperUpdate);
assert.deepEqual(paper.state().data.page_setup, paperPeer.state().data.page_setup);
assert.equal(paper.state().data.page_setup.orientation, "portrait");
assert.equal(paper.state().data.page_setup.margins.right, 49152);
paper.apply(p({ commands: [{ kind: "swap-page-orientation" }] }));
assert.equal(paper.state().data.page_setup.width, 862095);
assert.equal(paper.state().data.page_setup.orientation, "landscape");
paper.undo(); assert.equal(paper.state().data.page_setup.width, 609562);
paper.undo(); assert.equal(paper.state().data.page_setup.width, 420 * 1024);
assert.equal(paper.state().data.page_setup.margins.right, 49152);
paper.redo();
const validPaper = paper.state();
assert.throws(() => paper.set_page_setup(p({ width: 0 })), e => e.data.code === "edit.page-setup-invalid" && e.data.severity === "error");
assert.throws(() => paper.set_page_setup(p({ left: 2147483647 })), e => e.data.code === "edit.page-setup-invalid");
assert.deepEqual(paper.state(), validPaper);
paper.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "Paper setup survives WASM layout and packaging.", style: { families: null, size: null, line_height: null } }] }));
finish(paper);
assert.equal(paper.display_page(0).data.display.width, 609562);
const paperOpen = ws.open(p({ peer_id: "92" }), paper.save().data.bytes);
assert.deepEqual(paperOpen.state().data.page_setup, paper.state().data.page_setup);
const paperNative = paper.export(p("native")).data.content.bytes;
const paperTarget = ws.create(p({ document_id: "a9112233445566778899aabbccddeeff", peer_id: "93" }));
paperTarget.paste(p({ at: null }), paperNative);
assert.deepEqual(paperTarget.state().data.page_setup, paper.state().data.page_setup);
paperTarget.undo();
assert.equal(paperTarget.state().data.page_setup.patched, false);
assert.equal(paperTarget.state().data.blocks.length, 0);
paperTarget.redo(); finish(paperTarget);
assert.equal(paperTarget.display_page(0).data.display.width, 609562);
// Paragraph styles: define, apply, patch with clear, null alignment, State.
const styled = ws.create(p({ document_id: "60112233445566778899aabbccddeeff", peer_id: "41" }));
const para = styled.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "styled words in a justified paragraph", style: { families: null, size: null, line_height: null } }] })).data.blocks[0];
styled.apply(p({ commands: [
  { kind: "define-style", name: "Quote", parent: "", style: { families: null, size: null, line_height: null, indent_start: "24pt", space_before: "1lh", alignment: "justify", keep_together: true } },
  { kind: "apply-style", node: para, style: "Quote" },
  { kind: "set-style", node: para, style: { families: null, size: null, line_height: null, indent_first: "-12pt" }, clear: [] },
  { kind: "set-alignment", node: para, at: null, alignment: "end" },
  { kind: "set-alignment", node: para, at: null, alignment: null },
] }));
const styledState = styled.state().data;
const styledBlock = styledState.blocks[0];
assert.equal(styledBlock.style.name, "Quote");
assert.equal(styledBlock.style.direct.indent_first, "-12pt");
const quote = styledState.resolved[styledBlock.style.resolved];
assert.equal(quote.indent_start, 24 * 1024);
assert.equal(quote.indent_first, -12 * 1024);
assert.equal(quote.alignment, "justify");
assert.equal(quote.keep_together, true);
assert.ok(styledState.styles.some(s => s.name === "Heading 1" && s.builtin));
assert.throws(() => styled.apply(p({ commands: [{ kind: "delete-style", name: "" }] })));
finish(styled);
const reopened = ws.open(p({ peer_id: "42" }), styled.save().data.bytes);
assert.deepEqual(reopened.state().data.resolved, styledState.resolved);
reopened.free(); styled.free();
console.log("paragraph style WASM smoke passed");
// Plumbing: one workspace font as a Uint8Array shared by sessions, batch, atomic replace, copy, ranges.
{
  assert.equal(wasm.maxPacketBytes(), 32 * 1024 * 1024);
  const pw = new wasm.Workspace();
  const fontBytes = new Uint8Array(fs.readFileSync(path.join(__dirname, "../../../fixtures/fonts/AtkinsonHyperlegibleNext-Regular.ttf")));
  const face = pw.declare_font(p({ family: "Universal", weight: 400, style: "normal", stretch: 1000, face_index: 0 }), fontBytes).data;
  assert.throws(() => pw.declare_font(p({ family: "Universal", weight: 400, style: "normal", stretch: 1000, face_index: 0 }), [1, 2, 3]), e => e.data.code === "bindings.invalid");
  const plain = { families: null, size: null, line_height: null };
  const mk = (id, peer) => pw.create(p({ document_id: id, peer_id: peer }));
  const s1 = mk("b0112233445566778899aabbccddeeff", "101"), s2 = mk("b1112233445566778899aabbccddeeff", "102");
  const n1 = s1.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "hello world", style: plain }] })).data.blocks[0];
  s2.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "x", style: plain }] }));
  s1.apply(p({ commands: [{ kind: "format-text", node: n1, start: 0, end: 5, style: { families: ["Universal"], size: null, language: null, features: null, reset: false } }] }));
  finish(s1); finish(s2);
  assert.ok(!s1.diagnostics().data.some(d => d.code.startsWith("font.")), "the workspace face is used without a per-session declaration");
  assert.ok(s1.save().data.bytes.length > 0);
  // Batch: a failing third command applies nothing.
  const state = s1.state();
  const bad = p({ commands: [
    { kind: "insert-text", node: n1, at: 0, text: ">" },
    { kind: "insert-block", parent: null, index: 1, block_kind: "paragraph", text: "b", style: plain },
    { kind: "delete-text", node: n1, start: 500, end: 600 }], expected_revision: null, selection: null });
  assert.throws(() => s1.batch(bad), e => e.data.command === 2);
  assert.deepEqual(s1.state(), state);
  const stale = p({ commands: [{ kind: "insert-text", node: n1, at: 0, text: "!" }], expected_revision: [{ peer: "101", counter: 0 }], selection: null });
  assert.throws(() => s1.batch(stale), e => e.data.code === "bindings.stale");
  const sel = { anchor: { node: n1, offset: 6, affinity: "downstream" }, focus: { node: n1, offset: 11, affinity: "downstream" } };
  const report = s1.batch(p({ commands: [{ kind: "insert-text", node: n1, at: 0, text: ">> " }], expected_revision: state.data.revision, selection: sel }));
  assert.equal(report.data.selection.anchor.offset, 9);
  s1.undo();
  assert.equal(s1.state().data.blocks[0].text, "hello world");
  // Atomic replace: one undo step.
  const stable = s1.anchor_selection(p(sel)).data;
  const replaced = s1.replace_text(p({ selection: stable, text: "there", html: false }));
  assert.equal(s1.state().data.blocks[0].text, "hello there");
  assert.equal(replaced.data.selection.focus.offset, 11);
  s1.undo();
  assert.equal(s1.state().data.blocks[0].text, "hello world");
  assert.throws(() => s1.replace_selection(p({ selection: stable }), new Uint8Array([1, 2, 3])), e => e.data.code.startsWith("clipboard."));
  assert.equal(s1.state().data.blocks[0].text, "hello world");
  // Reading order carries byte ranges; marks carry anchor facts.
  finish(s1);
  const steps = s1.reading_order().data;
  assert.ok(steps.every(s => s.end >= s.start));
  assert.equal(steps[0].start, 0);
  assert.ok(Array.isArray(s1.diagnostics_for(p({ code_prefix: "font.", min_severity: null, subject: null, limit: 10 })).data.diagnostics));
  // Import as a copy: a new identity, no shared history.
  const saved = s1.save().data.bytes;
  const copy = pw.import_copy(p({ document_id: "b2112233445566778899aabbccddeeff", peer_id: "103" }), saved);
  assert.equal(copy.state().data.document_id, "b2112233445566778899aabbccddeeff");
  assert.equal(copy.state().data.blocks[0].text, "hello world");
  assert.deepEqual(copy.sync_info().data.vector.map(c => c.peer), ["103"]);
  assert.throws(() => copy.sync_import(s1.sync_export(p({ since: null }))), e => e.data.code === "bindings.copy-source");
  assert.throws(() => pw.import_copy(p({ document_id: "b0112233445566778899aabbccddeeff", peer_id: "104" }), saved), e => e.data.code === "bindings.id");
  copy.free(); s2.free(); s1.free(); pw.free();
  console.log("plumbing WASM smoke passed");
}
paperTarget.free(); paperOpen.free(); paperPeer.free(); paper.free(); ws.free();
console.log("page setup WASM smoke passed");

// Annotations: links and comment anchors (docs/annotations.md).
{
  const wa = new wasm.Workspace();
  const plain = { families: ["Source Serif Pro", "serif"], size: null, line_height: null };
  const a = wa.create(p({ document_id: "a0112233445566778899aabbccddeeff", peer_id: "120" }));
  const node = a.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "read the manual now", style: plain }] })).data.blocks[0];
  assert.throws(() => a.apply(p({ commands: [{ kind: "add-annotation", node, start: 9, end: 15, annotation: { kind: "link", target: { type: "url", url: "javascript:alert(1)" } } }] })), e => e.data.code === "annotation.link-scheme" && e.data.command === 0);
  assert.throws(() => a.apply(p({ commands: [{ kind: "add-annotation", node, start: 9, end: 15, annotation: { kind: "bookmark", name: "x" } }] })), e => e.data.code === "bindings.invalid");
  const added = a.apply(p({ commands: [
    { kind: "add-annotation", node, start: 9, end: 15, annotation: { kind: "link", target: { type: "url", url: "https://manual.example/?a=1&b=2" } } },
    { kind: "format-text", node, start: 9, end: 15, style: { decoration: { underline: true, strike: null }, color: [0, 0, 238, 255] } },
    { kind: "add-annotation", node, start: 0, end: 4, annotation: { kind: "comment", thread: "t-1" } },
  ] }));
  const [link, note] = added.data.annotations;
  assert.equal(added.data.annotations.length, 2);
  const state = a.state().data;
  assert.deepEqual(state.annotations.map(x => [x.kind, x.status, x.thread ?? null]), [["link", "active", null], ["comment", "active", "t-1"]]);
  assert.deepEqual(a.annotation(p(link)).data.spans, [{ node, start: 9, end: 15 }]);
  assert.equal(a.annotation(p(link)).data.payload.target.url, "https://manual.example/?a=1&b=2");
  assert.deepEqual(a.annotations_at(p({ node, at: 10 })).data.annotations.map(x => x.id), [link]);
  assert.deepEqual(a.annotations_in(p({ node, start: 0, end: 19 })).data.annotations.map(x => x.id), [note, link]);
  assert.deepEqual(a.annotations_of_kind(p("comment")).data.annotations.map(x => x.id), [note]);
  assert.throws(() => a.annotation_geometry(p(link)), e => e.data.code === "bindings.layout-required");
  finish(a);
  const geometry = a.annotation_geometry(p(link)).data;
  assert.ok(geometry.rects.length >= 1 && geometry.rects[0].corners.length === 4);
  assert.equal(geometry.top.page, 0);
  const display = a.display_page(0).data;
  assert.equal(display.links.length, 1);
  assert.equal(display.links[0].id, link);
  assert.deepEqual(display.links[0].target, { type: "url", url: "https://manual.example/?a=1&b=2" });
  assert.deepEqual(a.page_links(0).data.links, display.links);
  const displayWith = a.display_json(0).data;
  // A batch with a stale revision is refused; a good one maps nothing but applies.
  assert.throws(() => a.batch(p({ commands: [{ kind: "remove-annotation", id: link }], expected_revision: [{ peer: "120", counter: 0 }], selection: null })), e => e.data.code === "bindings.stale");
  a.batch(p({ commands: [{ kind: "update-annotation", id: link, annotation: { kind: "link", target: { type: "url", url: "https://other.example" } } }], expected_revision: a.state().data.revision, selection: null }));
  assert.equal(a.annotation(p(link)).data.payload.target.url, "https://other.example");
  a.undo();
  assert.equal(a.annotation(p(link)).data.payload.target.url, "https://manual.example/?a=1&b=2");
  // Save, reopen, copy: the annotations come along, copies get new identities.
  const bytes = a.save().data.bytes;
  const reopened = wa.open(p({ peer_id: "121" }), bytes); finish(reopened);
  assert.deepEqual(reopened.state().data.annotations, a.state().data.annotations);
  assert.equal(reopened.display_json(0).data, displayWith);
  const copy = wa.import_copy(p({ document_id: "a1112233445566778899aabbccddeeff", peer_id: "122" }), bytes);
  assert.equal(copy.state().data.annotations.length, 2);
  assert.ok(copy.state().data.annotations.every(x => x.id !== link && x.id !== note));
  // A comment whose text is deleted is orphaned; undo brings it back.
  a.apply(p({ commands: [{ kind: "delete-text", node, start: 0, end: 5 }] }));
  assert.equal(a.annotation(p(note)).data.status, "orphaned");
  a.undo();
  assert.equal(a.annotation(p(note)).data.status, "active");
  a.apply(p({ commands: [{ kind: "remove-annotation", id: link }] })); finish(a);
  assert.equal(a.annotation(p(link)).data, null);
  assert.equal(a.page_links(0).data.links.length, 0);
  assert.throws(() => a.page_links(99), e => e.data.code === "bindings.invalid");
  copy.free(); reopened.free(); a.free(); wa.free();
  console.log("annotations WASM smoke passed");
}

// Inline objects and fields (docs/inline.md).
{
  const wi = new wasm.Workspace();
  const plain = { families: null, size: null, line_height: null };
  const i = wi.create(p({ document_id: "b0112233445566778899aabbccddeeff", peer_id: "130" }));
  const hash = i.register_asset(p({ id: "red", kind: "image" }), new Uint8Array(fs.readFileSync(path.join(__dirname, "../../../fixtures/images/red-1x1.png")))).data;
  const para = i.apply(p({ commands: [{ kind: "insert-block", parent: null, index: 0, block_kind: "paragraph", text: "see  for page ", style: plain }] })).data.blocks[0];
  const picture = { kind: "image", align: "baseline", params: { asset: hash, alt: "a cat", width: "pt:" + 24 * 1024, height: "pt:" + 12 * 1024 } };
  // A bad object is refused whole and names the command.
  assert.throws(() => i.apply(p({ commands: [{ kind: "insert-inline", node: para, at: 4, object: { kind: "image", align: "baseline", params: { asset: "nope" } } }] })), e => e.data.code === "edit.invalid-command" && e.data.command === 0);
  const target = i.field_target(p({ node: para, at: 0 })).data;
  const end = Buffer.byteLength("see  for page ");
  i.apply(p({ commands: [
    { kind: "insert-inline", node: para, at: 4, object: picture },
    { kind: "insert-inline", node: para, at: end + 3, object: { kind: "field", align: "baseline", params: { field: "page-number" } } },
  ] }));
  const block = i.state().data.blocks[0];
  assert.equal(block.inline.length, 2);
  assert.deepEqual(block.inline.map(e => e.at), [4, end + 3]);
  assert.equal(block.inline[0].object.params.alt, "a cat");
  assert.throws(() => i.atom_rect(p({ node: para, at: 4 })), e => e.data.code === "bindings.layout-required" || e.data.code === "bindings.invalid");
  finish(i);
  const rect = i.atom_rect(p({ node: para, at: 4 })).data;
  assert.equal(rect.page, 0);
  assert.ok(rect.rect.width > 0 && rect.rect.height > 0);
  assert.throws(() => i.atom_rect(p({ node: para, at: 5 })), e => e.data.code === "bindings.invalid");
  assert.ok(i.display_json(0).data.includes('"type":"image"'));
  assert.deepEqual(i.diagnostics().data.filter(note => note.severity !== "info"), []);
  // A resize keeps the object's identity; undo takes it back.
  const id = block.inline[0].id;
  i.apply(p({ commands: [{ kind: "update-inline", node: para, at: 4, patch: { align: null, params: { width: "pt:" + 48 * 1024 } } }] }));
  assert.equal(i.state().data.blocks[0].inline[0].id, id);
  assert.equal(i.state().data.blocks[0].inline[0].object.params.width, "pt:" + 48 * 1024);
  i.undo(); assert.equal(i.state().data.blocks[0].inline[0].object.params.width, "pt:" + 24 * 1024);
  // A page-of with a real target, a copy of the document, and the package round trip.
  i.apply(p({ commands: [{ kind: "insert-inline", node: para, at: 0, object: { kind: "field", align: "baseline", params: { field: "page-of", target } } }] }));
  finish(i);
  assert.deepEqual(i.diagnostics().data.filter(note => note.severity !== "info"), []);
  const bytes = i.save().data.bytes;
  const again = wi.open(p({ peer_id: "131" }), bytes); finish(again);
  assert.equal(again.display_json(0).data, i.display_json(0).data);
  const copy = wi.import_copy(p({ document_id: "b1112233445566778899aabbccddeeff", peer_id: "132" }), bytes);
  assert.equal(copy.state().data.blocks[0].inline.length, 3);
  assert.ok(copy.state().data.blocks[0].inline.every(e => !i.state().data.blocks[0].inline.some(o => o.id === e.id)));
  const html = i.copy_as(p({ selection: { anchor: { node: para, offset: 0, affinity: "downstream" }, focus: { node: para, offset: Buffer.byteLength(i.state().data.blocks[0].text), affinity: "upstream" } }, format: "html" })).data;
  assert.ok(Buffer.from(html.content.bytes).toString("utf8").includes("<img "));
  copy.free(); again.free(); i.free(); wi.free();
  console.log("inline objects WASM smoke passed");
}
