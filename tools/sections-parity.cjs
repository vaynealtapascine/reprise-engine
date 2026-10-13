// Compare actual WASM facade output with native output from identical packages.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const wasm = require(path.resolve(process.argv[2], 'reprise_wasm.js'));
const dir = path.resolve(process.argv[3]);
const payload = data => ({ version: 1, data });
const workspace = new wasm.Workspace();
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
  doc.release_job(job); doc.free();
}
workspace.free();
console.log('sections native/WASM parity: geometry, continuous and vertical bands, notes and floats passed');
