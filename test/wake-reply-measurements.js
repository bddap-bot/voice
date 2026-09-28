import assert from 'node:assert/strict';
import { SIGN_OFF } from '../docs/identity.js';
import { includesPhrase } from '../docs/live.js';

const MEMORY = 'Context: Archived transcripts of completed conversations';
const FAREWELL = /\b(good ?bye|bye|good ?night|sleep (well|tight)|see you (later|soon|around)|talk (to you )?(later|soon)|end(ing)? (the|this) (session|conversation)|going (back )?to sleep|already asleep)\b/i;
const EVENTS = new Set(['session.started', 'session.input_transcript.delta', 'session.output_transcript.delta', 'session.delegation.created', 'session.instructions.append', 'session.instructions.appended', 'session.commentary.append', 'session.commentary.appended']);
const FIELDS = new Set(['type', 'raw', 'delta', 'content', 'event_id', 'client_event_id', 'start_ms', 'end_ms']);
const subject = (reply) => /(\w+)\W*$/.exec(reply)[1].toLowerCase();
const names = (text, reply) => new RegExp(`\\b${subject(reply)}\\b`, 'i').test(text);

export function spokenBetween(capture, ch, from, to) {
  return capture.rt.filter((row) => row.ch === ch && row.dir === 'in' && row.at >= from && row.at <= to && row.event.type === 'session.output_transcript.delta').map(({ event }) => event.delta).join('').trim();
}

export function spokenReply(capture, ch, stamp, end) {
  const applied = capture.rt.find((row) => row.ch === ch && row.dir === 'in' && row.event.type === 'session.commentary.appended' && row.event.client_event_id === `hub_${stamp}`);
  assert.ok(Number.isFinite(applied?.event.start_ms), `session ${ch}: reply was not applied to the audio timeline`);
  const sent = capture.rt.find((row) => row.ch === ch && row.dir === 'out' && row.event.event_id === `hub_${stamp}`);
  assert.ok(sent, `session ${ch}: missing reply submission`);
  return spokenBetween(capture, ch, sent.at, end);
}

export function recordedFixture({ replies, capture }) {
  const offer = (text) => {
    const { context, wake } = JSON.parse(text.slice(6));
    return `offer\n${JSON.stringify({ context, wake })}`;
  };
  return {
    replies,
    capture: {
      marks: capture.marks,
      frames: capture.frames.filter(({ dir, verb }) => verb === 'hub' || (dir === 'out' && verb === 'offer')).map((frame) => frame.verb === 'offer' ? { ...frame, text: offer(frame.text) } : frame),
      rt: capture.rt.filter(({ dir, event }) => dir === 'state' || EVENTS.has(event.type)).map((row) => ({ ...row, event: Object.fromEntries(Object.entries(row.event).filter(([key]) => FIELDS.has(key))) })),
    },
  };
}

export function assessWakeReplies({ replies, capture }, minimumWakes = 10) {
  assert.ok(replies.length > minimumWakes, 'not enough sleep/wake cycles');
  assert.equal(new Set(replies.map(subject)).size, replies.length, 'stub replies must distinguish sessions');
  assert.equal(capture.marks.filter((mark) => mark.kind === 'page-ready').length, 1, 'sessions must share one page');
  const offers = capture.frames.filter((frame) => frame.dir === 'out' && frame.verb === 'offer').map((frame) => JSON.parse(frame.text.slice(6)));
  assert.equal(offers.length, replies.length, 'one offer per session');
  const rows = [];
  for (const [ch, expected] of replies.entries()) {
    const [recall, recalled, begin, end, slept] = ['recall-start', 'recall-end', 'request-start', 'reply-end', 'sleep-start'].map((kind) => capture.marks.find((mark) => mark.kind === kind && mark.ch === ch));
    const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
    const started = incoming.find(({ event }) => event.type === 'session.started');
    assert.ok(started, `session ${ch}: never started`);
    const closed = capture.rt.find((row) => row.ch === ch && row.dir === 'state' && row.event.raw === 'close');
    assert.ok(closed, `session ${ch}: session never closed`);
    assert.ok(slept, `session ${ch}: never asked to sleep`);
    assert.ok(closed.at > slept.at, `session ${ch}: ended before the sleep request`);
    assert.ok(includesPhrase(spokenBetween(capture, ch, slept.at, closed.at), SIGN_OFF), `session ${ch}: did not sign off when asked to sleep`);
    assert.ok(begin && end && end.at > begin.at && slept.at >= end.at, `session ${ch}: missing reply window`);
    const greeting = spokenBetween(capture, ch, started.at, (recall ?? begin).at);
    assert.ok(greeting, `session ${ch}: did not greet`);
    assert.doesNotMatch(greeting, FAREWELL, `session ${ch}: opened with a farewell`);
    let answer = null;
    if (ch > 0) {
      assert.match(offers[ch].wake ?? '', /went to sleep about \d+ (second|minute|hour|day)s? ago\. You have just been woken/, `session ${ch}: missing wake marker`);
      const archived = offers[ch].context?.find((turn) => turn.speaker === 'user' && turn.text.startsWith(MEMORY));
      const turns = archived ? JSON.parse(archived.text.slice(archived.text.indexOf('\n') + 1)).at(-1).turns : [];
      assert.ok(turns.some((turn) => turn.speaker === 'live' && turn.text.includes(replies[ch - 1])), `session ${ch}: previous answer was not carried`);
      assert.ok(includesPhrase(turns.findLast((turn) => turn.speaker === 'live')?.text ?? '', SIGN_OFF), `session ${ch}: the carried conversation did not end on the sign-off`);
      assert.ok(recall && recalled && recalled.at > recall.at && begin.at >= recalled.at, `session ${ch}: missing recall window`);
      assert.ok(!incoming.some(({ at, event }) => at >= recall.at && at <= recalled.at && event.type === 'session.delegation.created'), `session ${ch}: delegated the recall question`);
      answer = spokenBetween(capture, ch, recall.at, recalled.at);
      assert.deepEqual(replies.filter((reply) => names(answer, reply)), [replies[ch - 1]], `session ${ch}: did not recall the answer from before the sleep`);
    }
    const hubs = capture.frames.filter((frame) => frame.verb === 'hub' && frame.at >= begin.at && frame.at < end.at).map((frame) => JSON.parse(frame.text.slice(4)));
    assert.equal(hubs.length, 1, `session ${ch}: expected one stub reply`);
    assert.ok(end.audio?.some((stats) => stats.bytesReceived > 0 && stats.totalSamplesReceived > 0), `session ${ch}: no received audio`);
    assert.ok(capture.rt.some((row) => row.ch === ch && row.dir === 'out' && row.event.type === 'session.commentary.append' && row.event.event_id === `hub_${hubs[0].stamp}` && row.event.content === expected), `session ${ch}: stub never reached Live`);
    const requestSpeech = spokenBetween(capture, ch, begin.at, end.at);
    const spoken = spokenReply(capture, ch, hubs[0].stamp, end.at);
    assert.equal(spoken, expected, `session ${ch}: spoke something other than the fresh hub reply`);
    for (const previous of replies.slice(0, ch)) assert.ok(!requestSpeech.includes(previous), `session ${ch}: repeated an earlier hub answer`);
    rows.push({ session: ch, greeting, recalled: answer, expected, spoken });
  }
  return rows;
}
