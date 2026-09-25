const SAFE_HREF = /^(#|https?:\/\/)/i;
const FORBIDDEN = new Set(['script', 'iframe', 'object', 'embed', 'image', 'link', 'meta', 'base', 'form', 'input', 'textarea', 'button']);

function sanitize(root) {
  for (const node of [root, ...root.querySelectorAll('*')]) {
    const name = node.localName.toLowerCase();
    if (FORBIDDEN.has(name) || (name === 'use' && !/^#/.test(node.getAttribute('href') ?? node.getAttribute('xlink:href') ?? '#'))) {
      if (node === root) throw new Error(`renderer output is a <${name}>`);
      node.remove(); continue;
    }
    for (const attribute of [...node.attributes]) {
      const key = attribute.name.toLowerCase();
      if (key.startsWith('on') || key === 'srcdoc' || key === 'formaction') node.removeAttribute(attribute.name);
      else if ((key === 'href' || key === 'xlink:href' || key === 'src') && !SAFE_HREF.test(attribute.value.trim())) node.removeAttribute(attribute.name);
    }
    if (name === 'a') { node.removeAttribute('target'); node.setAttribute('rel', 'noopener noreferrer'); }
  }
  return root;
}

export function adopt(target, markup, mime) {
  const parsed = new DOMParser().parseFromString(markup, mime);
  const root = mime === 'text/html' ? parsed.body : parsed.documentElement;
  if (root.querySelector('parsererror') || root.localName === 'parsererror') throw new Error('unparseable renderer output');
  sanitize(root);
  target.replaceChildren(...(mime === 'text/html' ? root.childNodes : [root]));
}

let mermaidReady;
export async function renderMermaid(target, source) {
  if (/\bimg\s*:/i.test(source)) throw new Error('mermaid image sources are not supported');
  mermaidReady ??= import('mermaid').then(({ default: mermaid }) => { mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark', layout: 'dagre', fontFamily: 'system-ui, sans-serif' }); return mermaid; });
  const mermaid = await mermaidReady;
  const { svg } = await mermaid.render(`diagram-${Math.random().toString(36).slice(2)}`, source);
  adopt(target, svg, 'image/svg+xml');
}

let katexReady;
export async function renderMath(target, source, display) {
  katexReady ??= import('katex').then(({ default: katex }) => {
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = new URL(KATEX_CSS, import.meta.url);
    document.head.append(link);
    return katex;
  });
  const katex = await katexReady;
  adopt(target, katex.renderToString(source, { displayMode: display, throwOnError: true, trust: false }), 'text/html');
}

const NUMBER = /^-?\d+(\.\d+)?(e[-+]?\d+)?$/i;
function csvRows(text) {
  const rows = [[]];
  let cell = '';
  let quoted = false;
  for (let index = 0; index < text.length; index++) {
    const character = text[index];
    if (quoted && character === '"' && text[index + 1] === '"') { cell += '"'; index++; }
    else if (character === '"' && (!cell || quoted)) quoted = !quoted;
    else if (!quoted && character === ',') { rows.at(-1).push(cell.trim()); cell = ''; }
    else if (!quoted && character === '\n') { rows.at(-1).push(cell.trim()); rows.push([]); cell = ''; }
    else if (character !== '\r') cell += character;
  }
  if (quoted) throw new Error('chart CSV: unclosed quote');
  rows.at(-1).push(cell.trim());
  return rows;
}
export function chartConfig(source, kind = 'line') {
  const text = source.trim();
  if (text.startsWith('{')) return JSON.parse(text);
  const rows = csvRows(text);
  const [header, ...body] = rows;
  const scatter = kind === 'scatter';
  const labels = body.map((row) => row[0]);
  const datasets = header.slice(1).map((label, column) => ({
    label,
    data: body.map((row) => (scatter ? { x: Number(row[0]), y: Number(row[column + 1]) } : Number(row[column + 1]))),
  }));
  if (!datasets.length || !body.length || body.some((row) => row.length !== header.length) || datasets.some((set) => set.data.some((value) => Number.isNaN(scatter ? value.x + value.y : value)))) throw new Error('chart CSV: header row then numeric rows');
  return { type: kind, data: scatter ? { datasets } : { labels, datasets } };
}

let chartReady;
export async function renderChart(target, source, kind) {
  const config = chartConfig(source, kind);
  chartReady ??= import('chart.js/auto').then(({ default: Chart }) => {
    Chart.defaults.color = '#c9c4bb';
    Chart.defaults.scale.grid.color = '#3a3a40';
    Chart.defaults.maintainAspectRatio = false;
    return Chart;
  });
  const Chart = await chartReady;
  if (target.rendererDisposed) return;
  const canvas = document.createElement('canvas');
  target.replaceChildren(canvas);
  const chart = new Chart(canvas, config);
  target.destroyRenderer = () => chart.destroy();
}

export function disposeRenderer(target) {
  target.rendererDisposed = true;
  target.destroyRenderer?.();
}
