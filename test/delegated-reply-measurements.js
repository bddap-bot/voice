import assert from 'node:assert/strict';

export const OVERLAP_MS = 400;
export const LEAD_MS = 3000;
export const STALL_MS = 1000;
const EVENTS = new Set(['session.started', 'session.input_transcript.delta', 'session.output_transcript.delta', 'session.delegation.created', 'session.commentary.append', 'session.commentary.appended']);
const FIELDS = new Set(['type', 'raw', 'delta', 'content', 'event_id', 'client_event_id', 'start_ms', 'end_ms']);
const words = (text) => text.toLowerCase().replace(/[’']/g, '').match(/[a-z]+/g) ?? [];
const subject = (reply) => words(reply).at(-1);

export function delegatedFixture({ utterance, replies, capture }) {
  return {
    utterance,
    replies,
    capture: {
      marks: capture.marks.filter(({ kind }) => ['utterance-start', 'utterance-end', 'reply-end'].includes(kind)),
      rt: capture.rt.filter(({ dir, event }) => dir === 'state' || EVENTS.has(event.type)).map((row) => ({ ...row, event: Object.fromEntries(Object.entries(row.event).filter(([key]) => FIELDS.has(key))) })),
    },
  };
}

export function assessDelegatedReplies({ utterance, replies, capture }) {
  const expected = words(utterance);
  const rows = [];
  for (const [ch, reply] of replies.entries()) {
    const [start, end, done] = ['utterance-start', 'utterance-end', 'reply-end'].map((kind) => capture.marks.find((mark) => mark.kind === kind && mark.ch === ch));
    assert.ok(start && end && done && start.at < end.at && end.at < done.at, `session ${ch}: missing utterance window`);
    const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
    const sent = capture.rt.find(({ ch: session, dir, at, event }) => session === ch && dir === 'out' && at >= start.at && at <= end.at && event.type === 'session.commentary.append' && event.content === reply);
    assert.ok(sent, `session ${ch}: the reply did not reach Live during the utterance`);
    const applied = incoming.find(({ event }) => event.type === 'session.commentary.appended' && event.client_event_id === sent.event.event_id);
    assert.ok(Number.isFinite(applied?.event.start_ms), `session ${ch}: the reply was not applied to the session timeline`);
    const heard = incoming.filter(({ at, event }) => at >= start.at && at <= done.at && event.type === 'session.input_transcript.delta');
    const transcript = words(heard.map(({ event }) => event.delta).join(' '));
    const covered = expected.filter((word) => transcript.includes(word)).length / expected.length;
    assert.ok(covered >= 0.8 && transcript.slice(-3).includes(expected.at(-1)), `session ${ch}: the utterance transcript is not whole: ${transcript.join(' ')}`);
    const starts = heard.map(({ event }) => event.start_ms);
    assert.ok(starts.every((at, index) => !index || at - starts[index - 1] <= STALL_MS), `session ${ch}: the microphone stalled mid-utterance`);
    const userEnd = Math.max(...starts);
    assert.ok(applied.event.start_ms <= userEnd - LEAD_MS, `session ${ch}: the reply reached the model too late in the utterance to test the turn`);
    const spoken = incoming.filter(({ at, event }) => at >= sent.at && at <= done.at && event.type === 'session.output_transcript.delta');
    const replyWords = new Set(words(reply).filter((word) => word.length > 3));
    const early = words(spoken.filter(({ event }) => event.start_ms < userEnd - OVERLAP_MS).map(({ event }) => event.delta).join(''));
    assert.deepEqual(early.filter((word) => replyWords.has(word)), [], `session ${ch}: spoke the reply over the utterance`);
    const said = spoken.map(({ event }) => event.delta).join('');
    assert.ok(words(said).includes(subject(reply)), `session ${ch}: never spoke the reply`);
    rows.push({ session: ch, heard: heard.map(({ event }) => event.delta).join(''), spoken: said.trim() });
  }
  return rows;
}
