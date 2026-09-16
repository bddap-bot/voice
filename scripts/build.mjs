import { build } from 'esbuild';
import { basename } from 'node:path';
import { rm } from 'node:fs/promises';

const shared = { bundle: true, minify: true, legalComments: 'none', format: 'esm', logLevel: 'warning' };

await build({ ...shared, entryPoints: ['src/puppet.js'], outfile: 'docs/puppet.js' });

await rm('docs/lib', { recursive: true, force: true });
const css = await build({
  ...shared,
  entryPoints: ['node_modules/katex/dist/katex.css'],
  outdir: 'docs/lib',
  entryNames: '[name]-[hash]',
  assetNames: '[name]-[hash]',
  loader: { '.woff2': 'file' },
  external: ['*.woff', '*.ttf'],
  metafile: true,
});
const katexCss = basename(Object.keys(css.metafile.outputs).find((name) => name.endsWith('.css')));

await build({
  ...shared,
  entryPoints: ['src/render.js'],
  outdir: 'docs/lib',
  splitting: true,
  chunkNames: '[name]-[hash]',
  alias: { katex: './node_modules/katex/dist/katex.mjs' },
  define: { KATEX_CSS: JSON.stringify(katexCss) },
});
