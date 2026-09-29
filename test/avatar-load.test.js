import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { traceStages } from '../docs/stage-trace.js';

const PHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1';
const ANDROID_PHONE = 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0.0.0 Mobile Safari/537.36';
const ANDROID_TABLET = 'Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36';
const DESKTOP = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36';

test('the README query prints per-stage p50 and p90 by device class from completed avatar loads', async () => {
  let clock = 1_790_000_000_000;
  const spans = [];
  const load = (userAgent, bytesMs, parseMs, failure) => traceStages((span) => spans.push(span), 'avatar-load', { 'avatar.file': 'a.vrm', 'user_agent.original': userAgent }, async (stage) => {
    await stage('bytes', () => { clock += bytesMs; });
    await stage('parse', () => { clock += parseMs; if (failure) throw new Error(failure); });
  }, () => clock).catch(() => {});
  for (const ms of [100, 400, 200, 300, 500, 600, 700, 800, 900, 1000]) await load(PHONE, ms, ms / 10);
  await load(DESKTOP, 40, 7);
  await load(ANDROID_PHONE, 450, 45);
  await load(ANDROID_TABLET, 60, 6);
  await load(DESKTOP, 5000, 5000, 'puppet load replaced');
  const resource = (service) => ({ attributes: [{ key: 'host.name', value: { stringValue: 'voice' } }, { key: 'service.name', value: { stringValue: service } }] });
  const input = [
    { resourceSpans: [{ resource: resource('page'), scopeSpans: [{ scope: { name: 'page' }, spans: spans.map((span) => ({ status: {}, ...span })) }] }] },
    { resourceSpans: [{ resource: resource('voice-live'), scopeSpans: [{ scope: { name: 'voice-live' }, spans: spans.map((span) => ({ ...span, spanId: 'f'.repeat(16) })) }] }] },
    { resourceLogs: [] },
  ].map((line) => JSON.stringify(line)).join('\n');
  const output = execFileSync('jq', ['-rn', '-f', new URL('../scripts/avatar-load.jq', import.meta.url).pathname], { input, encoding: 'utf8' });
  assert.deepEqual(output.trim().split('\n').map((row) => row.split('\t')), [
    ['class', 'stage', 'loads', 'p50_ms', 'p90_ms'],
    ['desktop', 'bytes', '1', '40', '40'],
    ['desktop', 'parse', '1', '7', '7'],
    ['phone', 'bytes', '11', '500', '900'],
    ['phone', 'parse', '11', '50', '90'],
    ['tablet', 'bytes', '1', '60', '60'],
    ['tablet', 'parse', '1', '6', '6'],
  ]);
});
