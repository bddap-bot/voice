import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import test from 'node:test';
import { chartConfig } from '../docs/lib/render.js';

test('a chart block parses CSV into a line config and scatter points, and refuses ragged rows', () => {
  assert.deepEqual(chartConfig('step,reward,loss\n"January, 2026",0.5,2\nFebruary,"0.75",1.5'), { type: 'line', data: { labels: ['January, 2026', 'February'], datasets: [{ label: 'reward', data: [0.5, 0.75] }, { label: 'loss', data: [2, 1.5] }] } });
  assert.deepEqual(chartConfig('x,y\n1,2\n3,4', 'scatter'), { type: 'scatter', data: { datasets: [{ label: 'y', data: [{ x: 1, y: 2 }, { x: 3, y: 4 }] }] } });
  assert.deepEqual(chartConfig('a,b\n1,2', 'bar').type, 'bar');
  assert.deepEqual(chartConfig(' {"type":"pie","data":{"labels":["a"],"datasets":[{"data":[1]}]}} ').type, 'pie');
  for (const bad of ['a,b\n1', 'a,b\n1,x', 'a\n1', 'a,b', 'a,b\n"1,2', '{']) assert.throws(() => chartConfig(bad), bad);
});

test('the initial page bundle references the renderers only through lazy imports of hashed files', async () => {
  const entry = await readFile(new URL('../docs/lib/render.js', import.meta.url), 'utf8');
  const lazy = [...entry.matchAll(/import\("\.\/([^"]+)"\)/g)].map((match) => match[1]);
  assert.equal(lazy.length, 3, entry);
  for (const name of lazy) assert.match(name, /-[A-Z0-9]{8}\.js$/);
  assert.ok(entry.length < 4096, `render.js is ${entry.length} bytes`);
  const entries = ['render.js', 'ort.js', 'transformers.js'];
  const files = (await readdir(new URL('../docs/lib', import.meta.url))).filter((name) => !entries.includes(name));
  for (const file of files) assert.match(file, /-[A-Z0-9]{8}\.(js|css|woff2|wasm)$/);
  const worker = await readFile(new URL('../docs/sw.js', import.meta.url), 'utf8');
  const hashed = new RegExp(/const HASHED = \/(.*)\/;/.exec(worker)[1]);
  assert.ok(files.every((name) => hashed.test(`/lib/${name}`)));
  for (const name of entries) assert.equal(hashed.test(`/lib/${name}`), false);
  assert.equal(hashed.test('/index.html'), false);
});
