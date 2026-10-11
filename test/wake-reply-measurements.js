import assert from 'node:assert/strict';
import { SIGN_OFF } from '../docs/identity.js';
import { includesPhrase } from '../docs/live.js';
import { applied, eventText, hubFrames, recordedRows, replySent } from './live-capture.js';

const MEMORY = 'Context: Archived transcripts of completed conversations';
const VOCALIZATION = /\[[a-z]+\]/g;
const FAREWELL = /\b(good ?bye|bye|good ?night|sleep (well|tight)|see you (later|soon|around)|talk (to you )?(later|soon)|end(ing)? (the|this) (session|conversation)|going (back )?to sleep|already asleep)\b/i;
const subject = (reply) => /(\w+)\W*$/.exec(reply)[1].toLowerCase();

export function spokenBetween(capture, ch, from, to) {
  return capture.rt.filter((row) => row.ch === ch && row.dir === 'in' && row.at >= from && row.at <= to && row.event.type === 'output_transcript.added').map(({ event }) => eventText(event)).join('').replace(VOCALIZATION, '').replace(/\s{2,}/g, ' ').trim();
}

export function spokenReply(capture, ch, stamp, end) {
  const hub = hubFrames(capture).find((frame) => frame.stamp === stamp);
  const sent = hub && replySent(capture, ch, hub);
  assert.ok(sent, `session ${ch}: stub never reached Live`);
  assert.ok(Number.isFinite(applied(capture, sent)?.event.start_ms), `session ${ch}: reply was not applied to the audio timeline`);
  return spokenBetween(capture, ch, sent.at, end);
}

export function recordedFixture({ replies, capture }) {
  const offer = (text) => {
    const { context, wake } = JSON.parse(text.slice(6));
    return `offer\n${JSON.stringify({ context, wake })}`;
  };
  return {
    replies,
    signOff: SIGN_OFF,
    capture: {
      marks: capture.marks,
      frames: capture.frames.filter(({ dir, verb }) => verb === 'hub' || (dir === 'out' && verb === 'offer')).map((frame) => frame.verb === 'offer' ? { ...frame, text: offer(frame.text) } : frame),
      rt: recordedRows(capture.rt),
    },
  };
}

export function assessWakeReplies({ replies, signOff, capture }, minimumWakes = 10) {
  assert.ok(replies.length > minimumWakes, 'not enough sleep/wake cycles');
  assert.equal(new Set(replies.map(subject)).size, replies.length, 'stub replies must distinguish sessions');
  assert.equal(capture.marks.filter((mark) => mark.kind === 'page-ready').length, 1, 'sessions must share one page');
  const offers = capture.frames.filter((frame) => frame.dir === 'out' && frame.verb === 'offer').map((frame) => JSON.parse(frame.text.slice(6)));
  assert.equal(offers.length, replies.length, 'one offer per session');
  const rows = [];
  for (const [ch, expected] of replies.entries()) {
    const [begin, end, slept] = ['request-start', 'reply-end', 'sleep-start'].map((kind) => capture.marks.find((mark) => mark.kind === kind && mark.ch === ch));
    const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
    const started = incoming.find(({ event }) => event.type === 'session.started');
    assert.ok(started, `session ${ch}: never started`);
    const closed = capture.rt.find((row) => row.ch === ch && row.dir === 'state' && row.event.raw === 'close');
    assert.ok(closed, `session ${ch}: session never closed`);
    assert.ok(slept, `session ${ch}: never asked to sleep`);
    assert.ok(closed.at > slept.at, `session ${ch}: ended before the sleep request`);
    assert.ok(includesPhrase(spokenBetween(capture, ch, slept.at, closed.at), signOff), `session ${ch}: did not sign off when asked to sleep`);
    assert.ok(begin && end && end.at > begin.at && slept.at >= end.at, `session ${ch}: missing reply window`);
    const greeting = spokenBetween(capture, ch, started.at, begin.at);
    assert.ok(greeting, `session ${ch}: did not greet`);
    assert.doesNotMatch(greeting, FAREWELL, `session ${ch}: opened with a farewell`);
    if (ch > 0) {
      assert.match(offers[ch].wake ?? '', /went to sleep about \d+ (second|minute|hour|day)s? ago\. You have just been woken/, `session ${ch}: missing wake marker`);
      const archived = offers[ch].context?.find((turn) => turn.speaker === 'user' && turn.text.startsWith(MEMORY));
      const turns = archived ? JSON.parse(archived.text.slice(archived.text.indexOf('\n') + 1)).at(-1).turns : [];
      assert.ok(turns.some((turn) => turn.speaker === 'live' && turn.text.includes(replies[ch - 1])), `session ${ch}: previous answer was not carried`);
      assert.ok(includesPhrase(turns.filter((turn) => turn.speaker === 'live').map((turn) => turn.text).join(' '), signOff), `session ${ch}: the carried conversation did not end on the sign-off`);
    }
    const hubs = hubFrames(capture).filter((frame) => frame.at >= begin.at && frame.at < end.at);
    assert.equal(hubs.length, 1, `session ${ch}: expected one stub reply`);
    assert.deepEqual(hubs[0].commentary, [expected], `session ${ch}: the stub carried another reply`);
    assert.ok(end.audio?.some((stats) => stats.bytesReceived > 0 && stats.totalSamplesReceived > 0), `session ${ch}: no received audio`);
    const requestSpeech = spokenBetween(capture, ch, begin.at, end.at);
    const spoken = spokenReply(capture, ch, hubs[0].stamp, end.at);
    assert.equal(spoken, expected, `session ${ch}: spoke something other than the fresh hub reply`);
    for (const previous of replies.slice(0, ch)) assert.ok(!requestSpeech.includes(previous), `session ${ch}: repeated an earlier hub answer`);
    rows.push({ session: ch, greeting, expected, spoken });
  }
  return rows;
}
