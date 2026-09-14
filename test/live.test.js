import assert from 'node:assert/strict';
import test from 'node:test';
import { AckUploader, AudioChunker, ConversationTrace, EventBatcher, SessionClock, TranscriptBatcher, audioFrame, errorEvent, formatElapsed, formatStartError, isOfferFor, shareFrame } from '../docs/live.js';

test('start errors include their name and first stack frame', () => {
  const error = new TypeError('Illegal invocation');
  error.stack = 'TypeError: Illegal invocation\n    at SessionClock.start (live.js:42:23)\n    at later (index.html:1:1)';
  assert.equal(formatStartError(error), 'TypeError: Illegal invocation — at SessionClock.start (live.js:42:23)');
  error.stack = 'SessionClock.start@https://example.test/live.js:42:23\nlater@https://example.test/index.html:1:1';
  assert.equal(formatStartError(error), 'TypeError: Illegal invocation — SessionClock.start@https://example.test/live.js:42:23');
});

test('only the matching SDP answer can resolve a replacement attempt', () => {
  const waiter = { id: 'new_offer' };
  assert.equal(isOfferFor(waiter, { offer_id: 'old_offer' }), false);
  assert.equal(isOfferFor(waiter, { offer_id: 'new_offer' }), true);
  assert.equal(isOfferFor(waiter, { id: 'new_offer' }), true);
  assert.equal(isOfferFor({ offerId: 'new_offer' }, { offer_id: 'new_offer' }), true);
});

test('share frame preserves a URL and carries image bytes after metadata', () => {
  const frame = shareFrame({ id: 'share_1', text: 'https://example.test/a?q=one', mime: 'image/png', image: Uint8Array.of(137, 80, 78, 71) });
  const boundary = new TextDecoder().decode(frame).indexOf('\n', 6);
  assert.deepEqual(JSON.parse(new TextDecoder().decode(frame.subarray(6, boundary))), { id: 'share_1', text: 'https://example.test/a?q=one', mime: 'image/png' });
  assert.deepEqual([...frame.subarray(boundary + 1)], [137, 80, 78, 71]);
  assert.throws(() => shareFrame({ id: 'share_2', text: '', mime: 'image/svg+xml', image: Uint8Array.of(1) }), /PNG, JPEG, GIF, or WebP/);
  assert.throws(() => shareFrame({ id: 'share_3', text: 'x'.repeat(8193) }), /text is too large/);
});

test('page errors preserve name message stack and session identity', () => {
  const error = new TypeError('forced failure');
  error.stack = 'TypeError: forced failure\n at page.js:1:2';
  assert.deepEqual(errorEvent(error, 'session_1', 42), { kind: 'error', session_id: 'session_1', name: 'TypeError', message: 'forced failure', stack: error.stack, at: 42 });
  assert.equal(new TextEncoder().encode(errorEvent(new Error('x'.repeat(3000))).message).length, 2048);
});

test('telemetry events flush together through one authenticated send', async () => {
  let flush;
  const sent = [];
  const batcher = new EventBatcher(async (events) => sent.push(events), { later: (fn) => { flush = fn; return 1; }, cancel: () => {} });
  batcher.add({ name: 'open' });
  batcher.add({ name: 'close' });
  flush();
  while (batcher.sending) await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, [[{ name: 'open' }, { name: 'close' }]]);
});

test('acknowledged transcript and audio frames retain their sequence and bytes', async () => {
  const sent = [];
  let uploader;
  uploader = new AckUploader(async (frame) => {
    sent.push(frame);
    const text = new TextDecoder().decode(frame);
    const metadata = JSON.parse(text.slice(text.indexOf('\n') + 1, text.startsWith('audio\n') ? text.indexOf('\n', 6) : undefined));
    const key = text.startsWith('audio\n') ? `audio:${metadata.session_id}:${metadata.side}:${metadata.seq}` : `transcript:${metadata.session_id}:${metadata.seq}`;
    queueMicrotask(() => uploader.ack(key));
  });
  const transcript = new TranscriptBatcher('session_2', uploader);
  transcript.add('user', 'hello', 1);
  transcript.add('model', 'hi', 2);
  transcript.flush();
  const audio = audioFrame({ sessionId: 'session_2', side: 'mic', seq: 0, bytes: Uint8Array.of(1, 2, 3) });
  uploader.add('audio:session_2:mic:0', audio);
  while (uploader.running) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(new TextDecoder().decode(sent[0]), 'transcript\n{"session_id":"session_2","seq":0,"turns":[{"at":1,"side":"user","text":"hello"},{"at":2,"side":"model","text":"hi"}]}');
  assert.deepEqual([...sent[1].slice(-3)], [1, 2, 3]);
});

test('audio chunks keep recorder order when blob conversion completes out of order', async () => {
  const frames = [];
  const uploader = { add: (key, frame) => { frames.push({ key, frame }); return true; } };
  let first;
  const chunker = new AudioChunker('session_3', 'model', uploader);
  const pending = chunker.add({ size: 1, arrayBuffer: () => new Promise((resolve) => { first = resolve; }) });
  chunker.add({ size: 1, arrayBuffer: async () => Uint8Array.of(2).buffer });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(frames.length, 0);
  first(Uint8Array.of(1).buffer);
  await pending;
  await chunker.tail;
  assert.deepEqual(frames.map(({ key, frame }) => [key, frame.at(-1)]), [
    ['audio:session_3:model:0', 1],
    ['audio:session_3:model:1', 2],
  ]);
});

test('large recorder blobs split below the relay limit without sequence gaps', async () => {
  const frames = [];
  const chunker = new AudioChunker('session_3', 'mic', { add: (key, frame) => { frames.push({ key, frame }); return true; } });
  await chunker.add(new Blob([new Uint8Array(1024 * 1024)]));
  assert.deepEqual(frames.map(({ key }) => key), [
    'audio:session_3:mic:0',
    'audio:session_3:mic:1',
    'audio:session_3:mic:2',
  ]);
  assert.ok(frames.every(({ frame }) => frame.byteLength < 512 * 1024));
});

test('a full upload queue retains transcript text and its sequence for retry', () => {
  let accepted = false;
  let retry;
  const frames = [];
  const transcript = new TranscriptBatcher('session_5', { add: (key, frame) => { frames.push({ key, frame }); return accepted; } }, { later: (fn) => { retry = fn; return 1; }, cancel: () => {} });
  transcript.add('user', 'keep me', 1);
  transcript.flush();
  accepted = true;
  retry();
  assert.deepEqual(frames.map(({ key }) => key), ['transcript:session_5:0', 'transcript:session_5:0']);
  assert.match(new TextDecoder().decode(frames[1].frame), /keep me/);
});

test('accepted transcript turns emit content-free telemetry', () => {
  const events = [];
  const transcript = new TranscriptBatcher('session_6', { add: () => true }, { onTurn: (side, at) => events.push({ side, at }) });
  transcript.add('user', 'private words', 7);
  transcript.add('model', 'private reply', 8);
  transcript.flush();
  assert.deepEqual(events, [{ side: 'user', at: 7 }, { side: 'model', at: 8 }]);
  assert.doesNotMatch(JSON.stringify(events), /private/);
});

test('retryable storage errors resend while permanent ones release only their matching frame', async () => {
  const sent = [];
  let uploader;
  uploader = new AckUploader(async () => {
    sent.push('sent');
    if (sent.length === 1) queueMicrotask(() => uploader.fail('transcript:session_4:0', true));
    else queueMicrotask(() => uploader.fail('transcript:session_4:0', false));
  }, { pause: async () => {}, timeout: 1000 });
  uploader.add('transcript:session_4:0', Uint8Array.of(1));
  while (uploader.running) await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, ['sent', 'sent']);
});

test('shared material and its hub reply remain in the conversation trace', () => {
  const trace = new ConversationTrace();
  trace.shared('share_1', 'https://example.test/a?q=one');
  assert.equal(trace.hub('share_1', 'received', 12), true);
  assert.deepEqual(trace.entries[0], { kind: 'delegation', id: 'share_1', sent: 'https://example.test/a?q=one', context: [], reply: 'received', timing: 12, shared: true });
});

test('ending voice does not cancel a pending shared request', () => {
  const trace = new ConversationTrace();
  trace.shared('share_pending', 'image');
  trace.heard('stop voice');
  trace.delegated('spoken_pending');
  trace.cancel();
  assert.equal(trace.entries[0].reply, '');
  assert.equal(trace.entries[2].reply, 'cancelled');
});

test('clock displays elapsed minutes and flips off at the configured cap', () => {
  let now = 1000;
  let tick;
  let label;
  let capped = 0;
  const clock = new SessionClock({ capSeconds: 30, now: () => now, every: (fn) => { tick = fn; return 42; }, cancel: () => {}, onTick: (seconds) => { label = formatElapsed(seconds); }, onCap: () => { capped++; } });
  clock.start();
  assert.equal(label, '00:00');
  now = 30999;
  tick();
  assert.equal(label, '00:29');
  assert.equal(capped, 0);
  now = 31000;
  tick();
  assert.equal(label, '00:30');
  assert.equal(capped, 1);
  tick();
  assert.equal(capped, 1);
});

test('timeline preserves every heard and spoken fragment around delegation', () => {
  const trace = new ConversationTrace();
  trace.heard('check the ', 100);
  trace.spoke('One moment.');
  trace.heard('deployment', 200);
  const delegation = trace.delegated('item_1', 500);
  trace.hub('item_1', 'running', 870);
  trace.spoke('It is running.');
  assert.equal(delegation.sent, 'check the\ndeployment');
  assert.equal(delegation.duration_ms, 400);
  assert.deepEqual(trace.entries, [
    { kind: 'heard', text: 'check the ' },
    { kind: 'spoken', source: 'model alone', text: 'One moment.' },
    { kind: 'heard', text: 'deployment' },
    { kind: 'delegation', id: 'item_1', sent: 'check the\ndeployment', context: [{ speaker: 'live', text: 'One moment.' }], reply: 'running', timing: 870, duration_ms: 400 },
    { kind: 'spoken', source: 'model after hub reply', text: 'It is running.' },
  ]);
});

test('direct response remains visible and later delegation cannot lose pending speech', () => {
  const trace = new ConversationTrace();
  trace.heard('first question');
  trace.spoke('Direct answer.');
  trace.heard('yes, do that');
  const entry = trace.delegated('item_2');
  assert.equal(entry.sent, 'first question\nyes, do that');
  assert.deepEqual(entry.context, [{ speaker: 'live', text: 'Direct answer.' }]);
  assert.equal(trace.entries[1].kind, 'spoken');
  assert.equal(trace.entries[1].source, 'model alone');
  assert.equal(trace.entries[1].text, 'Direct answer.');
});

test('delegation context drops oldest whole turns to fit the broker limit', () => {
  const trace = new ConversationTrace();
  for (let index = 0; index < 10; index++) {
    trace.heard(`question ${index} ${'q'.repeat(450)}`);
    trace.spoke(`answer ${index} ${'a'.repeat(450)}`);
  }
  trace.heard('check again');
  const entry = trace.delegated('item_context');
  assert.ok(new TextEncoder().encode(JSON.stringify(entry.context)).length <= 8192);
  assert.ok(entry.context.length < 20);
});

test('delegation request drops oldest complete fragments to fit the broker limit', () => {
  const trace = new ConversationTrace();
  for (let index = 0; index < 20; index++) {
    trace.heard(`question ${index} ${'q'.repeat(450)}`);
    trace.spoke('Still listening.');
  }
  const entry = trace.delegated('item_request');
  assert.ok(new TextEncoder().encode(entry.sent).length <= 8192);
  assert.match(entry.sent, /question 19/);
  assert.doesNotMatch(entry.sent, /question 0 /);
});

test('one uninterrupted delegation transcript is UTF-8 safely bounded', () => {
  const trace = new ConversationTrace();
  trace.heard(`prefix ${'🟢'.repeat(3000)} final words`);
  const sent = trace.delegated('item_long').sent;
  assert.ok(new TextEncoder().encode(sent).length <= 8192);
  assert.match(sent, /final words$/);
  assert.doesNotMatch(sent, /�/);
});

test('speech received after a hub reply keeps chronological attribution when input interleaves', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_overlap');
  trace.hub('item_overlap', 'running', 10);
  trace.spoke('It is ', 100, 200);
  trace.heard('sorry', 0, 150);
  trace.spoke('running.', 200, 300);
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'spoken').map((entry) => entry.source), ['model after hub reply', 'model after hub reply']);
});

test('speech after a later user turn is tagged model alone', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_later');
  trace.hub('item_later', 'running', 10);
  trace.spoke('Running.', 100, 200);
  trace.heard('thanks', 0, 300);
  trace.spoke('You are welcome.', 320, 400);
  assert.equal(trace.entries.at(-1).source, 'model alone');
});

test('hub results remain on their delegation when later speech occurs', () => {
  const trace = new ConversationTrace();
  trace.heard('deploy');
  trace.delegated('item_3');
  trace.heard('thanks');
  trace.spoke('Welcome.');
  trace.hub('item_3', 'deployed', 50);
  assert.equal(trace.entries[1].reply, 'deployed');
  assert.equal(trace.entries.at(-1).text, 'Welcome.');
});

test('failed and cancelled delegations remain visible', () => {
  const trace = new ConversationTrace();
  trace.heard('first');
  trace.delegated('item_4');
  trace.failed('item_4', 'hub reply timed out');
  trace.heard('second');
  trace.delegated('item_5');
  trace.cancel();
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'delegation').map(({ reply, failed }) => ({ reply, failed })), [
    { reply: 'hub reply timed out', failed: true },
    { reply: 'cancelled', failed: true },
  ]);
});
