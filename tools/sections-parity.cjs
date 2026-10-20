// Compare actual WASM facade output with native output from identical packages.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const wasm = require(path.resolve(process.argv[2], 'reprise_wasm.js'));
const dir = path.resolve(process.argv[3]);
const payload = data => ({ version: 1, data });
const workspace = new wasm.Workspace();
// The same section queries and placed navigation as the native example makes.
function queries(doc) {
  const state = doc.state().data;
  state.sections = state.sections ?? []; state.stories = state.stories ?? [];
  const pages = [];
  for (let page = 0; ; page++) {
    let label;
    try { label = doc.page_label(page).data; } catch (e) { break; }
    pages.push({ label, sections: doc.page_sections(page).data, regions: doc.page_regions(page).data, fields: doc.page_fields(page).data });
  }
  const sectionPages = {};
  for (const section of state.sections) sectionPages[section.id] = doc.section_pages(payload(section.id)).data;
  const instances = {};
  for (const story of state.stories) instances[story.id] = doc.region_instances(payload(story.id)).data;
  let navigation = null;
  const region = pages.length ? doc.page_regions(0).data[0] : undefined;
  if (region) {
    const hit = doc.hit_placed(payload({ page: 0, x: region.rect.x + Math.trunc(region.rect.width / 2), y: region.rect.y + Math.trunc(region.rect.height / 2), mode: null })).data;
    if (hit.kind === 'running') {
      const anchor = { ...hit.position, offset: 0 };
      navigation = { hit, caret: doc.caret_in(payload({ instance: hit.instance, position: hit.position })).data,
        selection: doc.selection_rects_in(payload({ instance: hit.instance, selection: { anchor, focus: hit.position } })).data };
    } else navigation = { hit };
  }
  return { pages, section_pages: sectionPages, instances, navigation };
}
for (const test of JSON.parse(fs.readFileSync(path.join(dir, 'native.json'), 'utf8'))) {
  const doc = workspace.open(payload({ peer_id: '2' }), new Uint8Array(fs.readFileSync(path.join(dir, `${test.name}.rpr`))));
  const job = doc.start_layout(payload(test.options)).data;
  let pages;
  for (let unit = 0; unit < 100_000; unit++) {
    if (unit % 7 === 0) assert.equal(doc.step(job, 0).data.used, 0);
    const progress = doc.step(job, 1).data;
    if (progress.complete) { pages = progress.pages; break; }
  }
  assert.ok(pages, `${test.name}: bounded WASM job must finish`);
  assert.deepEqual(pages.map(page => doc.display_json(page).data), test.display, `${test.name}: native/WASM display bytes`);
  assert.deepEqual(doc.diagnostics().data, test.diagnostics, `${test.name}: diagnostics`);
  assert.deepEqual(doc.state().data.sections, test.sections, `${test.name}: section state`);
  assert.deepEqual(queries(doc), test.queries, `${test.name}: section queries and navigation`);
  doc.release_job(job); doc.free();
}
workspace.free();
console.log('sections native/WASM parity: geometry, continuous and vertical bands, notes and floats, section queries and navigation passed');
