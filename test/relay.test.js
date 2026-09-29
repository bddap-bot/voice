import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const pinned = {
  'botq_dash_wasm.js': 'dd3d9b589c1f01f485f960431f13b557b81496377193eb96e953414625673c6a',
  'botq_dash_wasm_bg.wasm': '6c6d5786f9d64d03cb1c22564805dce6a020f5110e4f05483e4d48f71e640df3',
};

test('vendored relay transport matches its pinned build', async () => {
  for (const [name, digest] of Object.entries(pinned)) {
    const bytes = await readFile(new URL(`../docs/relay/${name}`, import.meta.url));
    assert.equal(createHash('sha256').update(bytes).digest('hex'), digest, name);
  }
});
