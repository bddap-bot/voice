import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import test from 'node:test';
import { chartConfig } from '../docs/lib/render.js';

test('a chart block parses CSV into a line config and scatter points, and refuses ragged rows', () => {
  assert.deepEqual(chartConfig('step,reward,loss\n1,0.5,2\n2,0.75,1.5'), { type: 'line', data: { labels: ['1', '2'], datasets: [{ label: 'reward', data: [0.5, 0.75] }, { label: 'loss', data: [2, 1.5] }] } });
  assert.deepEqual(chartConfig('x,y\n1,2\n3,4', 'scatter'), { type: 'scatter', data: { datasets: [{ label: 'y', data: [{ x: 1, y: 2 }, { x: 3, y: 4 }] }] } });
  assert.deepEqual(chartConfig('a,b\n1,2', 'bar').type, 'bar');
  assert.deepEqual(chartConfig(' {"type":"pie","data":{"labels":["a"],"datasets":[{"data":[1]}]}} ').type, 'pie');
  for (const bad of ['a,b\n1', 'a,b\n1,x', 'a\n1', 'a,b', '{']) assert.throws(() => chartConfig(bad), bad);
});

test('the initial page bundle references the renderers only through lazy imports of hashed files', async () => {
  const entry = await readFile(new URL('../docs/lib/render.js', import.meta.url), 'utf8');
  const lazy = [...entry.matchAll(/import\("\.\/([^"]+)"\)/g)].map((match) => match[1]);
  assert.equal(lazy.length, 3, entry);
  for (const name of lazy) assert.match(name, /-[A-Z0-9]{8}\.js$/);
  assert.ok(entry.length < 4096, `render.js is ${entry.length} bytes`);
  const files = await readdir(new URL('../docs/lib', import.meta.url));
  for (const file of files.filter((name) => name !== 'render.js')) assert.match(file, /-[A-Z0-9]{8}\.(js|css|woff2)$/);
  const worker = await readFile(new URL('../docs/sw.js', import.meta.url), 'utf8');
  const hashed = new RegExp(/const HASHED = \/(.*)\/;/.exec(worker)[1]);
  assert.ok(files.filter((name) => name !== 'render.js').every((name) => hashed.test(`/lib/${name}`)));
  assert.equal(hashed.test('/lib/render.js'), false);
  assert.equal(hashed.test('/index.html'), false);
});
