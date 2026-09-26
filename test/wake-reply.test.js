import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { assessWakeReplies } from './wake-reply-measurements.js';

const recorded = () => JSON.parse(fs.readFileSync(process.env.VOICE_WAKE_REPLY_CAPTURE ?? new URL('./fixtures/wake-reply-capture.json', import.meta.url), 'utf8'));

test('recorded carried-context wakes speak each fresh hub reply exactly', () => {
  const rows = assessWakeReplies(recorded());
  assert.equal(rows.filter((row) => row.carried).length, 10);
});

for (let session = 1; session <= 10; session++) {
  test(`a prior answer substituted in woken session ${session} fails the live oracle`, () => {
    const data = recorded();
    const begin = data.capture.rt.find((row) => row.ch === session && row.dir === 'out' && row.event.type === 'session.commentary.append' && row.event.event_id?.startsWith('hub_')).at;
    const end = data.capture.marks.find((mark) => mark.kind === 'reply-end' && mark.ch === session).at;
    const deltas = data.capture.rt.filter((row) => row.ch === session && row.dir === 'in' && row.at >= begin && row.at <= end && row.event.type === 'session.output_transcript.delta');
    assert.ok(deltas.length);
    deltas.forEach((row, index) => { row.event.delta = index === 0 ? data.replies[session - 1] : ''; });
    assert.throws(() => assessWakeReplies(data), (error) => error.message.includes(`session ${session}: spoke something other than the fresh hub reply`) && error.actual === data.replies[session - 1]);
  });
}

for (const [label, mutate, error] of [
  ['missing carried answer', (data) => { const frame = data.capture.frames.filter((row) => row.verb === 'offer')[1]; const offer = JSON.parse(frame.text.slice(6)); offer.context = []; frame.text = 'offer\n' + JSON.stringify(offer); }, /previous answer was not carried/],
  ['page reload', (data) => data.capture.marks.push({ kind: 'page-ready' }), /sessions must share one page/],
  ['missing sleep', (data) => { data.capture.rt = data.capture.rt.filter((row) => row.ch !== 1 || row.dir !== 'state' || row.event.raw !== 'close'); }, /session never closed/],
  ['extra speech', (data) => { data.capture.rt.find((row) => row.ch === 1 && row.event.type === 'session.output_transcript.delta' && row.event.delta.includes('violet')).event.delta += ' The hub will answer later.'; }, /spoke something other than the fresh hub reply/],
]) {
  test(`${label} cannot produce a passing wake measurement`, () => {
    const data = recorded();
    mutate(data);
    assert.throws(() => assessWakeReplies(data), error);
  });
}
