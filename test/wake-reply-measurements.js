import assert from 'node:assert/strict';

export function spokenReply(capture, ch, stamp, end) {
  const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
  const applied = incoming.find(({ event }) => ['session.instructions.appended', 'session.commentary.appended'].includes(event.type) && event.client_event_id === `hub_${stamp}`);
  assert.ok(Number.isFinite(applied?.event.start_ms), `session ${ch}: reply was not applied to the audio timeline`);
  const sent = capture.rt.find((row) => row.ch === ch && row.dir === 'out' && row.event.event_id === `hub_${stamp}`);
  assert.ok(sent, `session ${ch}: missing reply submission`);
  return incoming.filter(({ event, at }) => at >= sent.at && at <= end && event.type === 'session.output_transcript.delta').map(({ event }) => event.delta).join('').trim();
}

export function assessWakeReplies({ replies, capture }, minimumWakes = 10) {
  assert.ok(replies.length > minimumWakes, 'not enough sleep/wake cycles');
  assert.equal(new Set(replies).size, replies.length, 'stub replies must distinguish sessions');
  assert.equal(capture.marks.filter((mark) => mark.kind === 'page-ready').length, 1, 'sessions must share one page');
  const offers = capture.frames.filter((frame) => frame.dir === 'out' && frame.verb === 'offer').map((frame) => JSON.parse(frame.text.slice(6)));
  assert.equal(offers.length, replies.length, 'one offer per session');
  const rows = [];
  for (const [ch, expected] of replies.entries()) {
    const begin = capture.marks.find((mark) => mark.kind === 'request-start' && mark.ch === ch);
    const end = capture.marks.find((mark) => mark.kind === 'reply-end' && mark.ch === ch);
    assert.ok(begin && end && end.at > begin.at, `session ${ch}: missing reply window`);
    const hubs = capture.frames.filter((frame) => frame.verb === 'hub' && frame.at >= begin.at && frame.at < end.at).map((frame) => ({ ...JSON.parse(frame.text.slice(4)), at: frame.at }));
    assert.equal(hubs.length, 1, `session ${ch}: expected one stub reply`);
    assert.ok(end.audio?.some((stats) => stats.bytesReceived > 0 && stats.totalSamplesReceived > 0), `session ${ch}: no received audio`);
    const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
    assert.ok(incoming.some(({ event }) => event.type === 'session.started'), `session ${ch}: never started`);
    assert.ok(capture.marks.some((mark) => mark.kind === 'sleep-start' && mark.ch === ch && mark.at >= end.at), `session ${ch}: missing sleep action`);
    assert.ok(capture.rt.some((row) => row.ch === ch && row.at > end.at && row.dir === 'state' && row.event.raw === 'close'), `session ${ch}: session never closed`);
    assert.ok(capture.rt.some((row) => row.ch === ch && row.dir === 'out' && row.event.type === 'session.commentary.append' && row.event.event_id === `hub_${hubs[0].stamp}` && row.event.content === expected), `session ${ch}: stub never reached Live`);
    if (ch > 0) {
      assert.match(offers[ch].wake, /went to sleep.*just been woken/, `session ${ch}: missing wake marker`);
      const archived = offers[ch].context.find((turn) => turn.speaker === 'user' && turn.text.startsWith('Context: Archived transcript of completed conversations'));
      const turns = archived ? JSON.parse(archived.text.slice(archived.text.indexOf('\n') + 1)) : [];
      assert.ok(turns.some((turn) => turn.speaker === 'live' && turn.text.includes(replies[ch - 1])), `session ${ch}: previous answer was not carried`);
    }
    const requestSpeech = incoming.filter(({ event, at }) => at >= begin.at && at <= end.at && event.type === 'session.output_transcript.delta').map(({ event }) => event.delta).join('');
    const spoken = spokenReply(capture, ch, hubs[0].stamp, end.at);
    assert.equal(spoken, expected, `session ${ch}: spoke something other than the fresh hub reply`);
    for (const previous of replies.slice(0, ch)) assert.ok(!requestSpeech.includes(previous), `session ${ch}: repeated an earlier hub answer`);
    rows.push({ session: ch, carried: ch > 0, expected, spoken });
  }
  return rows;
}
