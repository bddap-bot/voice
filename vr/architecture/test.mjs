import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const { ARCHITECTURES, GENERATOR } = process.env;
const scratch = mkdtempSync(join(tmpdir(), 'architecture-'));

function generate(description) {
  const file = join(scratch, 'made.json');
  writeFileSync(file, typeof description === 'string' ? description : JSON.stringify(description));
  const run = spawnSync('python3', [GENERATOR, scratch, file], { encoding: 'utf8' });
  return { ...run, path: join(scratch, 'made.glb') };
}

function refused(description) {
  const run = generate(description);
  assert.notEqual(run.status, 0, 'the generator refuses it');
  return run.stderr;
}

function load(path) {
  const bytes = readFileSync(path);
  const length = bytes.readUInt32LE(12);
  const gltf = JSON.parse(bytes.subarray(20, 20 + length).toString());
  const binary = bytes.subarray(20 + length + 8);
  const view = gltf.bufferViews[gltf.extensions.VOICE_program.bufferView];
  const module = new WebAssembly.Module(binary.subarray(view.byteOffset, view.byteOffset + view.byteLength));
  const { exports } = new WebAssembly.Instance(module, {});
  const places = new Map(gltf.nodes.map((node, at) => [node.name, at]));
  const object = {
    gltf,
    module,
    feed(text) {
      const bytes = Buffer.from(text);
      const at = exports.data(bytes.length);
      if (at) new Uint8Array(exports.memory.buffer).set(bytes, at);
      return at !== 0;
    },
    tick(time, tip) {
      const at = exports.tick(time, ...(tip ?? [0, 0, 0]), tip ? 1 : 0);
      object.records = new Float32Array(exports.memory.buffer, at, gltf.nodes.length * 10);
    },
    settle(time, tip) {
      for (let tick = 0; tick < 300; tick++) object.tick(time, tip);
    },
    node(name) {
      assert.ok(places.has(name), `a node named ${name}`);
      const record = Array.from(object.records.subarray(places.get(name) * 10, places.get(name) * 10 + 10));
      return { at: record.slice(0, 3), scale: record.slice(7) };
    },
    rest(name) {
      assert.ok(places.has(name), `a node named ${name}`);
      const node = gltf.nodes[places.get(name)];
      return { at: node.translation ?? [0, 0, 0], scale: node.scale ?? [1, 1, 1], node };
    },
    has: name => places.has(name),
  };
  object.tick(0);
  return object;
}

const colour = (object, name) => object.gltf.materials[object.gltf.meshes[object.rest(name).node.mesh].primitives[0].material].pbrMetallicRoughness.baseColorFactor;
const ready = name => load(join(ARCHITECTURES, `${name}.glb`));
const close = (actual, expected, message) => assert.ok(Math.abs(actual - expected) <= 1e-3 * Math.max(1, Math.abs(expected)), `${message}: ${actual} against ${expected}`);
const worn = (object, module, prefix = '') => object.gltf.nodes.map(node => node.name).filter(name => name.startsWith(`${prefix}module${module}.`) && /\.(chunk\d+|any)$/.test(name) && object.node(name).scale[0] > 0.5).map(name => name.split('.').pop());
const thick = (object, module, chunks, prefix = '') => Array.from({ length: chunks }, (_, chunk) => (object.has(`${prefix}tie${module}.${chunk}`) ? object.node(`${prefix}tie${module}.${chunk}`).scale[1] : 0));

test('every ready description is an object whose parts are named and whose program imports nothing', () => {
  const parts = { transformer: ['stream', 'token', 'bank', 'module5', 'chunk5'], reuse: ['stream', 'module7', 'chunk3'], loop: ['module7', 'chunk4', 'pass0', 'pass1'], streams: ['one.stream', 'two.stream1', 'two.stream2', 'two.token2', 'two.module5'], budget: ['reuse.chunk3', 'shallow.module3', 'narrow.chunk7', 'full.module7'] };
  for (const [name, names] of Object.entries(parts)) {
    const object = ready(name);
    for (const part of names) object.rest(part);
    assert.deepEqual(WebAssembly.Module.imports(object.module), []);
    assert.deepEqual(object.gltf.extensionsUsed, ['VOICE_program']);
    assert.equal(object.gltf.extensionsRequired, undefined);
    assert.ok(object.gltf.nodes.length <= 4096);
    assert.equal(new Set(object.gltf.nodes.map(node => node.name)).size, object.gltf.nodes.length, 'no two nodes share a name');
  }
  assert.equal(ready('reuse').gltf.extensions.VOICE_program.duration, 10, 'a token takes a second to enter, one a module, and one to leave');
});

test('an object is as large as the overlay shows one', () => {
  for (const name of ['transformer', 'budget']) {
    const { gltf } = ready(name);
    const low = [Infinity, Infinity, Infinity];
    const high = [-Infinity, -Infinity, -Infinity];
    const visit = (at, origin, size) => {
      const node = gltf.nodes[at];
      const scale = (node.scale ?? [1, 1, 1]).map(factor => factor * size);
      const here = (node.translation ?? [0, 0, 0]).map((value, axis) => origin[axis] + value * size);
      if (node.mesh !== undefined && !node.rotation) {
        const bounds = gltf.accessors[gltf.meshes[node.mesh].primitives[0].attributes.POSITION];
        for (let axis = 0; axis < 3; axis++) {
          low[axis] = Math.min(low[axis], here[axis] + bounds.min[axis] * scale[axis], here[axis] + bounds.max[axis] * scale[axis]);
          high[axis] = Math.max(high[axis], here[axis] + bounds.min[axis] * scale[axis], here[axis] + bounds.max[axis] * scale[axis]);
        }
      }
      for (const child of node.children ?? []) visit(child, here, size);
    };
    for (const root of gltf.scenes[0].nodes) visit(root, [0, 0, 0], 1);
    const longest = Math.max(...high.map((value, axis) => value - low[axis]));
    assert.ok(longest > 0.24 && longest <= 0.2501, `${name} is ${longest} m on its longest side`);
  }
});

test('stored weights are solid plates in the bank and each use of them is a hollow frame on the stream', () => {
  const reuse = ready('reuse');
  const plates = reuse.rest('bank').node.children.map(child => reuse.gltf.nodes[child].name);
  assert.deepEqual(plates, ['chunk0', 'chunk1', 'chunk2', 'chunk3'], 'four chunks are stored once each');
  for (let module = 0; module < 8; module++) {
    assert.deepEqual(worn(reuse, module), [`chunk${module % 4}`], `module ${module} wears its chunk's colour`);
    assert.deepEqual(thick(reuse, module, 4).map(value => value > 0.5), [0, 1, 2, 3].map(chunk => chunk === module % 4), `module ${module} is tied to its chunk alone`);
    assert.deepEqual(colour(reuse, `module${module}.chunk${module % 4}`), colour(reuse, `chunk${module % 4}`), `module ${module}'s frame is its plate's colour`);
  }
  assert.notDeepEqual(colour(reuse, 'chunk0'), colour(reuse, 'chunk1'), 'plates differ in colour');
  const plain = ready('transformer');
  for (let module = 0; module < 6; module++) assert.deepEqual(worn(plain, module), [`chunk${module}`]);
  assert.equal(plain.rest('bank').node.children.length, 6);
});

test('a looped middle is the same chunks worn again, with unshared ends', () => {
  const loop = ready('loop');
  assert.deepEqual(Array.from({ length: 8 }, (_, module) => worn(loop, module)[0]), ['chunk0', 'chunk1', 'chunk2', 'chunk3', 'chunk1', 'chunk2', 'chunk3', 'chunk4']);
  const [first, second] = [loop.rest('pass0'), loop.rest('pass1')];
  close(first.at[1], (loop.rest('module1').at[1] + loop.rest('module3').at[1]) / 2, 'the first pass is marked beside modules 1 to 3');
  close(second.at[1], (loop.rest('module4').at[1] + loop.rest('module6').at[1]) / 2, 'the second pass is marked beside modules 4 to 6');
  loop.settle(0);
  close(loop.node('pass1').scale[0], 1, 'and stays marked while the loop is the map in use');
  const run = generate({ models: [{ width: 64, modules: 8, chunks: 5, maps: ['mod', { loop: { ends: 1, times: 2 } }] }] });
  assert.equal(run.status, 0, run.stderr);
  const later = load(run.path);
  assert.ok(later.rest('loop.pass0').scale[0] > 0, 'a bracket out of use still has a place to point at');
  later.settle(0);
  assert.ok(later.node('loop.pass0').scale[0] < 1e-3, 'a loop that is not in use shows no brackets');
  later.feed('map loop');
  later.settle(0);
  close(later.node('loop.pass0').scale[0], 1, 'and shows them once it is');
});

test('a description that does not hold together is refused by name', () => {
  const model = { width: 256, modules: 8 };
  assert.match(refused({ models: [] }), /at least one model/);
  assert.match(refused({ models: [{ ...model, depth: 3 }] }), /unknown \['depth'\]/);
  assert.match(refused({ models: [{ modules: 8 }] }), /width must be a positive number/);
  assert.match(refused({ models: [{ ...model, chunks: 4, maps: ['own'] }] }), /own needs a chunk for each of the 8 modules, not 4/);
  assert.match(refused({ models: [{ ...model, chunks: 4, maps: [{ table: [0, 1, 2, 3, 4, 0, 1, 2] }] }] }), /a table names one of the 4 chunks for each of the 8 modules/);
  assert.match(refused({ models: [{ ...model, chunks: 5, maps: [{ loop: { ends: 1, times: 3 } }] }] }), /is 11 modules, not 8/);
  assert.match(refused({ models: [{ ...model, chunks: 4, maps: [{ random: 1 }, { random: 2 }] }] }), /two maps share a name/);
  assert.match(refused({ models: [{ ...model, chunks: 4, maps: [{ table: [0, 1, 2] }] }] }), /a table names one of the 4 chunks for each of the 8 modules/);
  assert.match(refused({ models: [{ ...model, chunks: 2, maps: [{ loop: { ends: 1, times: 2 } }] }] }), /modules, not 8/);
  assert.match(refused({ models: [{ ...model, modules: 2.5 }] }), /modules must be a whole number from 1 to 256/);
  assert.match(refused({ models: [{ ...model, modules: 100000 }] }), /modules must be a whole number from 1 to 256/);
  assert.match(refused({ models: [{ ...model, name: 'a' }, { ...model, name: 'b', width: 7 }] }), /width 7 is under 1\/32 of the widest model's 256/);
  assert.match(refused({ models: [model, model] }), /several models each need their own name/);
  assert.match(refused({ models: [{ ...model, name: 'a' }, { ...model, name: 'a' }] }), /several models each need their own name/);
  assert.match(refused({ models: [{ ...model, name: 'shared' }] }), /is no word live data uses/);
  assert.match(refused({ models: [{ ...model, name: 'Wide' }] }), /a name starts with a lower-case letter/);
  assert.match(refused({ models: [{ ...model, chunks: 4, maps: [{ random: 1, name: 'all' }] }] }), /a map's name starts with a lower-case letter/);
  assert.match(refused({ models: [{ ...model, mix: [] }] }), /mix belongs to a model of two streams/);
  assert.match(refused({ models: [{ ...model, streams: 2 }] }), /such a model needs it/);
  assert.match(refused({ models: [{ ...model, streams: 2, mix: Array.from({ length: 8 }, (_, module) => [1, 1, 1, module === 7 ? 2 : 1]) }] }), /four weights from 0 to 1/);
  assert.match(refused({ models: [{ ...model, streams: 3 }] }), /streams must be 1 or 2/);
  assert.match(refused('{"models": [{"width": 256, "modules": 8, "streams": 2.0, "mix": []}]}'), /streams must be 1 or 2, not 2\.0/);
  assert.match(refused({ models: [{ width: 64, modules: 48, chunks: 48, maps: ['pick'] }] }), /would have \d+ nodes.*over the 4096/);
  assert.match(refused({ models: [{ width: 64, modules: 40, chunks: 40, maps: ['pick'] }] }), /and 2\d{5} triangles, over the 4096, 600000 or 200000 a host draws/);
});

test('a random map is fixed by its seed', () => {
  const description = seed => ({ models: [{ width: 64, modules: 6, chunks: 3, maps: [{ random: seed }] }] });
  const made = seed => {
    const run = generate(description(seed));
    assert.equal(run.status, 0, run.stderr);
    const object = load(run.path);
    return Array.from({ length: 6 }, (_, module) => worn(object, module)[0]);
  };
  assert.deepEqual(made(3), made(3));
  assert.notDeepEqual(made(3), made(4));
});

test('time carries a token up the stream, and the chunk a module uses visits its frame', () => {
  const plain = ready('transformer');
  for (const module of [0, 3, 5]) {
    plain.settle(module + 1.5);
    const frame = plain.rest(`module${module}`).at;
    close(plain.node('token').at[1], frame[1], `held at ${module + 1.5} s the token is level with module ${module}`);
    for (let chunk = 0; chunk < 6; chunk++) {
      const visitor = plain.node(`visit${chunk}`);
      if (chunk !== module) {
        assert.equal(visitor.scale[0], 0, `chunk ${chunk} stays in the bank`);
        continue;
      }
      close(visitor.scale[0], 1, 'its copy is whole');
      close(visitor.at[0], frame[0], 'and sits in the frame');
      close(visitor.at[1], frame[1], 'at its height');
    }
    assert.ok(plain.node(`tie${module}.${module}`).scale[1] > 2, 'the tie it came along is thick');
    close(plain.node('bead').scale[0], 1, 'a bead is inside the module');
    close(plain.node('bead').at[1], frame[1], 'half way from the lower rung to the upper');
    assert.equal(plain.node(`pip${module}.${module}`).scale[0] > 0.1, false, 'what the module adds has not joined the token yet');
  }
  plain.settle(6.9);
  for (let module = 0; module < 6; module++) assert.ok(plain.node(`pip${module}.${module}`).scale[0] > 0.9, `past the last module the token carries what module ${module} added`);
  plain.settle(-3.5);
  close(plain.node('token').at[1], plain.rest('module3').at[1], 'time below zero runs an earlier token');
});

test('map switches which chunk each module uses', () => {
  const reuse = ready('reuse');
  reuse.feed('map random');
  reuse.settle(0);
  const table = Array.from({ length: 8 }, (_, module) => worn(reuse, module));
  assert.ok(table.every(variants => variants.length === 1 && variants[0] !== 'any'));
  assert.notDeepEqual(table.flat(), Array.from({ length: 8 }, (_, module) => `chunk${module % 4}`), 'the random map is not the mod map');
  for (let module = 0; module < 8; module++) {
    const chunk = Number(table[module][0].slice(5));
    assert.deepEqual(thick(reuse, module, 4).map(value => Math.round(value)), [0, 1, 2, 3].map(other => +(other === chunk)));
  }
  reuse.feed('map nothing');
  reuse.settle(0);
  assert.deepEqual(Array.from({ length: 8 }, (_, module) => worn(reuse, module)), table, 'a map the model lacks changes nothing');
  reuse.feed('map mod');
  reuse.settle(0);
  assert.deepEqual(worn(reuse, 5), ['chunk1']);
});

test('a module that picks per token wears no chunk and takes one as each token arrives', () => {
  const reuse = ready('reuse');
  reuse.feed('map pick');
  reuse.settle(0);
  for (let module = 0; module < 8; module++) {
    assert.deepEqual(worn(reuse, module), ['any']);
    for (const tie of thick(reuse, module, 4)) assert.ok(tie > 0.1 && tie < 0.6, 'every chunk is faintly within reach');
  }
  const picks = new Set();
  for (let token = 0; token < 8; token++) {
    reuse.settle(token * 10 + 3.5);
    const ties = thick(reuse, 2, 4);
    const taken = ties.filter(tie => tie > 1);
    assert.equal(taken.length, 1, 'one chunk is taken');
    const chunk = ties.findIndex(tie => tie > 1);
    close(reuse.node(`visit${chunk}`).at[1], reuse.rest('module2').at[1], 'and it is the one that visits');
    picks.add(chunk);
  }
  assert.ok(picks.size > 1, 'tokens differ in what they pick');
  const taken = () => Array.from({ length: 8 }, (_, module) => {
    reuse.settle(module + 1.5);
    return thick(reuse, module, 4).findIndex(tie => tie > 1);
  });
  const first = taken();
  assert.ok(new Set(first).size > 1, 'and modules differ for one token');
  reuse.feed('token 3');
  const fourth = taken();
  assert.notDeepEqual(fourth, first, 'token names which token runs while time is held within one');
  reuse.settle(3 * 10 + 1.5);
  reuse.feed('token 0');
  assert.deepEqual(Array.from({ length: 8 }, (_, module) => { reuse.settle(30 + module + 1.5); return thick(reuse, module, 4).findIndex(tie => tie > 1); }), fourth, 'the same token that time alone reaches');
});

test('weigh restacks plates and frames into columns as tall as what is stored and what is computed', () => {
  const budget = ready('budget');
  budget.feed('weigh');
  budget.settle(0);
  const column = (model, part, count) => {
    const [first, second] = [budget.node(`${model}.${part}0`), budget.node(`${model}.${part}1`)];
    return (second.at[1] - first.at[1]) * count;
  };
  const stored = { reuse: column('reuse', 'chunk', 4), shallow: column('shallow', 'chunk', 4), narrow: column('narrow', 'chunk', 8), full: column('full', 'chunk', 8) };
  const computed = { reuse: column('reuse', 'module', 8), shallow: column('shallow', 'module', 4), narrow: column('narrow', 'module', 8), full: column('full', 'module', 8) };
  close(stored.reuse / stored.shallow, 1, 'eight modules on four chunks store what four modules do');
  close(stored.full / stored.shallow, 2, 'eight unshared modules store twice that');
  close(stored.narrow / stored.shallow, (8 * 184 ** 2) / (4 * 256 ** 2), 'eight modules at width 184 store 3% more');
  close(computed.reuse / computed.full, 1, 'sharing computes as much as the unshared eight');
  close(computed.reuse / computed.shallow, 2, 'twice the four');
  close(computed.narrow / computed.shallow, (8 * 184 ** 2) / (4 * 256 ** 2), 'and the narrow eight about as much as the four');
  for (const model of ['reuse', 'narrow']) {
    const cell = budget.node(`${model}.module3`).scale;
    close(cell[0] * cell[1], 1, `${model}'s frame keeps its area as it becomes a cell`);
    assert.equal(budget.node(`${model}.token`).scale[0] < 1e-3, true, 'no token runs while weighing');
    assert.ok(thick(budget, 3, 8, `${model}.`).every(tie => tie < 1e-3), 'and the ties are put away');
    assert.ok(budget.node(`${model}.module3.read`).scale[0] < 1e-3 && budget.node(`${model}.module3.add`).scale[0] < 1e-3, 'with the rungs');
  }
  close(budget.node('reuse.module0').scale[0], budget.node('narrow.module0').scale[0] * 184 / 256, 'cells of every width are equally wide');
  budget.feed('weigh off');
  budget.settle(0);
  close(budget.node('full.chunk7').at[1], budget.rest('full.chunk7').at[1], 'weigh off puts the bank back');
});

test('shared lifts one chunk and every module that uses it', () => {
  const reuse = ready('reuse');
  reuse.feed('shared 1');
  reuse.settle(0);
  const lifted = name => reuse.node(name).at[2] > 0.005;
  assert.deepEqual([0, 1, 2, 3].map(chunk => lifted(`chunk${chunk}`)), [false, true, false, false]);
  assert.deepEqual(Array.from({ length: 8 }, (_, module) => lifted(`module${module}.chunk${module % 4}`)), [false, true, false, false, false, true, false, false]);
  assert.ok(reuse.node('module0.chunk0').scale[0] < 0.85, 'the rest draw back');
  close(reuse.node('module1').at[2], 0, 'a lifted frame leaves its rungs on the stream');
  reuse.feed('shared off');
  reuse.settle(0);
  assert.ok(!lifted('chunk1') && reuse.node('module0.chunk0').scale[0] > 0.99);
  reuse.feed('shared');
  const seen = new Set();
  for (let tick = 0; tick < 800; tick++) {
    reuse.tick(0);
    seen.add([0, 1, 2, 3].findIndex(chunk => lifted(`chunk${chunk}`)));
  }
  assert.deepEqual([...seen].filter(chunk => chunk >= 0).sort(), [0, 1, 2, 3], 'alone it walks through every chunk');
});

test('a tip at a plate or a frame lifts that chunk while it stays', () => {
  const reuse = ready('reuse');
  const tip = name => [reuse.rest('model').at[0] + reuse.rest(name).at[0], reuse.rest(name).at[1], 0];
  reuse.settle(0, tip('chunk2'));
  assert.ok(reuse.node('module6.chunk2').at[2] > 0.005 && reuse.node('module2.chunk2').at[2] > 0.005 && reuse.node('module1.chunk1').at[2] < 1e-4);
  reuse.settle(0, tip('module7'));
  assert.ok(reuse.node('chunk3').at[2] > 0.005 && reuse.node('chunk2').at[2] < 1e-4);
  reuse.settle(0);
  assert.ok(reuse.node('chunk3').at[2] < 1e-4, 'and lets go when the tip leaves');
  const budget = ready('budget');
  const at = (model, part) => [budget.node(model).at[0] + budget.node(`${model}.${part}`).at[0], budget.node(`${model}.${part}`).at[1], 0];
  budget.settle(0, at('full', 'chunk2'));
  assert.ok(budget.node('full.chunk2').at[2] > 0.003 && budget.node('reuse.chunk2').at[2] < 1e-4, 'a tip finds the plate of a model further along the row');
  budget.feed('weigh');
  budget.settle(0);
  budget.settle(0, at('full', 'chunk6'));
  assert.deepEqual(Array.from({ length: 8 }, (_, chunk) => budget.node(`full.chunk${chunk}`).at[2] > 0.003), Array.from({ length: 8 }, (_, chunk) => chunk === 6), 'and, weighed, the cell it is at');
  budget.settle(0, at('reuse', 'module5'));
  assert.ok(budget.node('reuse.chunk1').at[2] > 0.003 && budget.node('reuse.chunk3').at[2] < 1e-4, 'or the chunk of the frame cell it is at');
  budget.settle(0, at('narrow', 'chunk6'));
  assert.deepEqual(Array.from({ length: 8 }, (_, chunk) => budget.node(`narrow.chunk${chunk}`).at[2] > 0.002), Array.from({ length: 8 }, (_, chunk) => chunk === 6), 'the nearest cell where several are in reach');
});

test('show keeps the named models side by side and puts the rest away', () => {
  const budget = ready('budget');
  budget.feed('show reuse full');
  budget.settle(0);
  assert.deepEqual(['reuse', 'shallow', 'narrow', 'full'].map(model => Math.round(budget.node(model).scale[0])), [1, 0, 0, 1]);
  const gap = budget.rest('shallow').at[0] - budget.rest('reuse').at[0];
  close(budget.node('full').at[0] - budget.node('reuse').at[0], gap, 'the two stand as close as neighbours');
  close(budget.node('full').at[0] + budget.node('reuse').at[0], budget.rest('full').at[0] + budget.rest('reuse').at[0], 'about the middle of the row');
  budget.feed('show nobody');
  budget.settle(0);
  assert.deepEqual(['reuse', 'shallow', 'narrow', 'full'].map(model => Math.round(budget.node(model).scale[0])), [1, 0, 0, 1], 'a show that names no model changes nothing');
  budget.feed('show all');
  budget.settle(0);
  for (const model of ['reuse', 'shallow', 'narrow', 'full']) close(budget.node(model).at[0], budget.rest(model).at[0], `${model} is back in its place`);
});

test('two streams each carry a token, and a module reaches each by its mix', () => {
  const run = generate({ models: [{ width: 128, modules: 2, streams: 2, mix: [[1, 0.2, 0.5, 1], [0.3, 1, 1, 0.1]] }] });
  assert.equal(run.status, 0, run.stderr);
  const object = load(run.path);
  const girth = name => {
    const bounds = object.gltf.accessors[object.gltf.meshes[object.rest(name).node.mesh].primitives[0].attributes.POSITION];
    return bounds.max[1] - bounds.min[1];
  };
  assert.ok(girth('module0.read2') < 0.5 * girth('module0.read1'), 'a rung is thinner for a smaller weight');
  assert.ok(girth('module1.write2') < 0.5 * girth('module1.write1') && girth('module0.write1') < girth('module0.write2'), 'for writes as for reads');
  object.settle(2.5);
  close(object.node('token1').at[1], object.node('token2').at[1], 'the tokens rise together');
  assert.ok(object.node('token2').at[0] > object.rest('module0').at[0] && object.node('token1').at[0] < object.rest('module0').at[0], 'one each side of the modules');
  object.settle(3.9);
  assert.ok(object.node('pip1.12').scale[0] < 0.7 * object.node('pip1.11').scale[0] && object.node('pip1.12').scale[0] > 0, 'what a module adds to each stream is smaller for a smaller write');
  object.settle(2.5);
  assert.ok(object.node('bead1').scale[0] < 0.7 * object.node('bead2').scale[0] && object.node('bead1').scale[0] > 0, 'and what it reads from each is smaller for a smaller read');
});

test('a command takes only the words that are its own', () => {
  const reuse = ready('reuse');
  const lifted = () => [0, 1, 2, 3].filter(chunk => reuse.node(`chunk${chunk}`).at[2] > 0.005);
  reuse.feed('shared chunk2');
  reuse.settle(0);
  assert.deepEqual(lifted(), [2], 'a chunk may be named as its node is');
  reuse.feed('shared 9');
  reuse.settle(0);
  assert.deepEqual(lifted(), [2], 'a chunk no model stores changes nothing');
  reuse.feed('map; weigh');
  reuse.settle(0);
  assert.ok(reuse.node('chunk3').at[1] < reuse.rest('chunk3').at[1], 'map with no name leaves the next command alone');
  assert.deepEqual(worn(reuse, 5), ['chunk1']);
  reuse.feed('weigh off shared off');
  reuse.settle(0);
  assert.equal(reuse.feed(`map pick ${'x'.repeat(1100)}`), false, 'data longer than the program has room for is dropped whole');
  reuse.settle(0);
  assert.deepEqual(worn(reuse, 5), ['chunk1']);
  const budget = ready('budget');
  budget.feed('show narrow; shared 2');
  budget.settle(0);
  assert.deepEqual(['reuse', 'shallow', 'narrow', 'full'].map(model => Math.round(budget.node(model).scale[0])), [0, 0, 1, 0], 'show ends at a word that is no model');
  assert.ok(budget.node('narrow.chunk2').at[2] > 0.002);
});

test('a model far narrower than its neighbour keeps hollow frames', () => {
  const run = generate({ models: [{ name: 'wide', width: 4096, modules: 2 }, { name: 'thin', width: 256, modules: 2 }] });
  assert.equal(run.status, 0, run.stderr);
  const object = load(run.path);
  const across = name => {
    const bounds = object.gltf.accessors[object.gltf.meshes[object.rest(name).node.mesh].primitives[0].attributes.POSITION];
    return bounds.max[0] - bounds.min[0];
  };
  for (const model of ['wide', 'thin']) {
    const [frame, hole] = [across(`${model}.module0.chunk0`), across(`${model}.visit0`)];
    assert.ok(hole > 0.3 * frame && hole < frame, `${model}'s frame is ${frame} across around a hole of ${hole}`);
  }
  close(across('thin.chunk0') / across('wide.chunk0'), 256 / 4096, 'and its plate is as much smaller as its width');
});

test('several commands in one message each take effect, and no time or tip makes a transform that is not finite', () => {
  const budget = ready('budget');
  budget.feed('show narrow, reuse; shared 2; weigh');
  budget.settle(4.2);
  assert.equal(Math.round(budget.node('full').scale[0]), 0);
  assert.ok(budget.node('reuse.chunk2').at[2] > 0.003 && budget.node('reuse.chunk3').at[1] < budget.rest('reuse.chunk3').at[1]);
  for (const name of ['transformer', 'reuse', 'loop', 'streams', 'budget']) {
    const object = ready(name);
    for (const [at, data] of ['', 'map pick', 'weigh', 'shared', 'show all', 'weigh off shared off map mod', '\u0000ÿ;;', 'shared 99999999999999999999'].entries()) {
      if (data) object.feed(data);
      for (let step = -40; step < 400; step++) {
        object.tick(step * 0.073 * (at + 1), step % 3 ? undefined : [0.01 * (step % 25), 0.006 * (step % 41), 0]);
        assert.ok(object.records.every(Number.isFinite), `${name} after ${JSON.stringify(data)} at step ${step}`);
      }
    }
  }
});
