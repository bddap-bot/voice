import assert from 'node:assert/strict';
import test from 'node:test';
import { AckUploader, AudioChunker, ConversationTrace, EventBatcher, TranscriptBatcher, audioFrame, checkedClientEvent, contextEvent, errorEvent, formatElapsed, formatStartError, includesPhrase, isOfferFor, usageLimit, withoutTrailingPhrase, shareFrame } from '../docs/live.js';
import { HAND_OFF_PHRASE, SIGN_OFF } from '../docs/identity.js';

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

test('page errors preserve failure and client identity', () => {
  const error = new TypeError('forced failure');
  error.stack = 'TypeError: forced failure\n at page.js:1:2';
  assert.deepEqual(errorEvent(error, 'session_1', 42, { userAgent: 'Test Browser/1.0', webgpuAdapter: true }), { kind: 'error', session_id: 'session_1', name: 'TypeError', message: 'forced failure', stack: error.stack, user_agent: 'Test Browser/1.0', webgpu_adapter: true, at: 42 });
  assert.equal(new TextEncoder().encode(errorEvent(new Error('x'.repeat(3000))).message).length, 2048);
  assert.equal(errorEvent(error).webgpu_adapter, null);
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

test('a retryable rejection that arrives while its frame is still sending is retried without an unhandled rejection', async () => {
  const unhandled = [];
  const record = (reason) => unhandled.push(reason);
  process.on('unhandledRejection', record);
  let release;
  const sending = new Promise((resolve) => { release = resolve; });
  const sent = [];
  let uploader;
  uploader = new AckUploader(async () => {
    sent.push('sent');
    if (sent.length === 1) return sending;
    queueMicrotask(() => uploader.ack('transcript:session_5:0'));
  }, { pause: async () => {}, timeout: 1000 });
  uploader.add('transcript:session_5:0', Uint8Array.of(1));
  uploader.fail('transcript:session_5:0', true);
  await new Promise((resolve) => setImmediate(resolve));
  release();
  while (uploader.running) await new Promise((resolve) => setImmediate(resolve));
  process.off('unhandledRejection', record);
  assert.deepEqual(unhandled, []);
  assert.deepEqual(sent, ['sent', 'sent']);
});

test('a hub reply records its spoken text on its delegation', () => {
  const trace = new ConversationTrace();
  const heard = trace.heard('check the queue');
  assert.equal(heard, trace.entries[0]);
  trace.delegated('item_channels');
  assert.equal(trace.hub('item_channels', ['Two jobs.', 'Both run.'], 9), true);
  assert.deepEqual(['reply', 'timing'].map((key) => trace.entries[1][key]), ['Two jobs. Both run.', 9]);
});

test('the sign-off is found in a transcript regardless of case, spacing and punctuation', () => {
  for (const text of [SIGN_OFF, SIGN_OFF.toLowerCase().replace(/\.$/, ''), `Goodnight. ${SIGN_OFF.replace(/\.$/, '!')}`, SIGN_OFF.replace(' ', '\n').replace(/\.$/, ' —')]) assert.equal(includesPhrase(text, SIGN_OFF), true, text);
  for (const text of ['Goodnight, Corvus.', SIGN_OFF.split(' ').slice(0, -1).join(' '), SIGN_OFF.replace(' ', ' never '), SIGN_OFF.slice(0, -3)]) assert.equal(includesPhrase(text, SIGN_OFF), false, text);
});

test('a session is over its window at twice the window of turns, and continues from the last window', () => {
  const trace = new ConversationTrace();
  trace.window = 3;
  trace.opened(0);
  const over = [];
  for (let index = 0; index < 4; index++) {
    trace.heard(`q${index}`);
    trace.delegated(`item_${index}`);
    over.push(trace.overWindow());
    trace.spoke(`a${index}`);
    over.push(trace.overWindow());
  }
  assert.deepEqual(over, [false, false, false, false, false, true, true, true]);
  const { context, wake } = trace.continuation();
  assert.deepEqual(context, [{ speaker: 'live', text: 'a2' }, { speaker: 'user', text: 'q3' }, { speaker: 'live', text: 'a3' }]);
  assert.match(wake, /continues the conversation/);
  trace.opened(context.length);
  assert.equal(trace.sessionTurns(), 3);
  assert.equal(trace.overWindow(), false);
});

test('an ended session closes its undelegated turn, so the next session hands the hub only its own words', () => {
  const trace = new ConversationTrace();
  trace.heard('Goodnight, Corvus.');
  trace.delegated('item_answered');
  trace.hub('item_answered', ['Noted.'], 5);
  trace.heard('Goodnight, ');
  trace.heard('Corvus.');
  trace.slept();
  const next = trace.heard('What is new?');
  assert.equal(next.text, 'What is new?');
  assert.equal(trace.delegated('item_next').sent, 'What is new?');
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'delegation').map(({ id, failed }) => [id, Boolean(failed)]), [['item_answered', false], ['item_next', false]]);
});

test('an ended session keeps its last spoken turn apart from the next session greeting', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_before_sleep');
  trace.hub('item_before_sleep', ['The beacon is amber.'], 5);
  trace.spoke('The beacon is amber.');
  trace.slept();
  trace.spoke('Hello again.');
  assert.deepEqual(trace.context().slice(-2), [{ speaker: 'live', text: 'The beacon is amber.' }, { speaker: 'live', text: 'Hello again.' }]);
  assert.equal(trace.entries.at(-1).source, 'model alone');
  trace.heard('status again');
  trace.delegated('item_unspoken');
  trace.hub('item_unspoken', ['The beacon is violet.'], 5);
  trace.slept();
  trace.spoke('Hello once more.');
  assert.equal(trace.entries.at(-1).source, 'model alone');
});

test('a trace snapshot drops its oldest turns past the limit and restores with the same memory', () => {
  const trace = new ConversationTrace();
  trace.heard('first question');
  trace.spoke('First answer.');
  trace.slept(1000);
  trace.heard('second question');
  trace.spoke('Second answer.');
  trace.slept(2000);
  const restored = new ConversationTrace(() => {}, JSON.parse(JSON.stringify(trace.snapshot())));
  assert.deepEqual(restored.entries, trace.entries);
  assert.deepEqual(restored.wake(3000), trace.wake(3000));
  const newest = trace.snapshot(trace.entries.slice(2).reduce((sum, entry) => sum + JSON.stringify(entry).length, 0));
  assert.deepEqual(newest, { entries: trace.entries.slice(2), sleeps: [{ index: 0, at: 1000 }, { index: 2, at: 2000 }] });
  const second = new ConversationTrace();
  second.heard('second question');
  second.spoke('Second answer.');
  second.slept(2000);
  assert.deepEqual(new ConversationTrace(() => {}, newest).wake(3000), second.wake(3000));
});

test('shared material and its hub reply remain in the conversation trace', () => {
  const trace = new ConversationTrace();
  trace.shared('share_1', 'https://example.test/a?q=one');
  assert.equal(trace.hub('share_1', ['received'], 12), true);
  assert.deepEqual(trace.entries[0], { kind: 'delegation', id: 'share_1', sent: 'https://example.test/a?q=one', context: [], reply: 'received', timing: 12, shared: true });
});

test('pending holds only the words heard since the last delegation and leaves them pending', () => {
  const trace = new ConversationTrace();
  trace.heard('check the beacon', 100);
  trace.delegated('first', 200);
  trace.spoke('Checking.');
  trace.heard(' order filament ', 300);
  assert.deepEqual(trace.pending(500), { sent: 'order filament', context: [{ speaker: 'user', text: 'check the beacon' }, { speaker: 'live', text: 'Checking.' }], duration_ms: 200 });
  assert.equal(trace.delegated('second', 500).sent, 'order filament');
  assert.equal(trace.pending().sent, '');
});

test('a request ended with the hand-off phrase carries every user turn since the last such request or sleep, without the phrase', () => {
  assert.equal(HAND_OFF_PHRASE, 'Fly, Corvus');
  assert.equal(withoutTrailingPhrase('Check the printer. Okay, fly corvus!', HAND_OFF_PHRASE), 'Check the printer. Okay');
  assert.equal(withoutTrailingPhrase('Fly, Corvus, check the printer.', HAND_OFF_PHRASE), 'Fly, Corvus, check the printer.');
  assert.equal(withoutTrailingPhrase('Corvus', HAND_OFF_PHRASE), 'Corvus');
  assert.equal(withoutTrailingPhrase('Compare the evalFly Corvus', HAND_OFF_PHRASE), 'Compare the eval');
  const trace = new ConversationTrace();
  trace.heard('Egdod issue fourteen', 100);
  trace.delegated('a', 200);
  trace.spoke('Checking.');
  trace.heard('give me a Chesterton', 300);
  assert.equal(trace.delegated('b', 400).sent, 'give me a Chesterton');
  trace.heard('fence analysis. Fly, Corvus.', 500);
  assert.equal(trace.endsRequest(), true);
  assert.equal(trace.delegated('c', 600).sent, 'Egdod issue fourteen\ngive me a Chesterton\nfence analysis.');
  assert.equal(trace.endsRequest(), false);
  trace.heard('Check the printer', 700);
  assert.equal(trace.delegated('d', 800).sent, 'Check the printer');
  trace.heard('and the deck. Fly corvus', 900);
  assert.equal(trace.delegated('e', 1000).sent, 'Check the printer\nand the deck.');
  trace.heard('Fly, Corvus.', 1100);
  assert.equal(trace.delegated('f', 1200).sent, 'Fly, Corvus.');
  trace.heard('before the sleep', 1300);
  trace.slept(1400);
  trace.heard('after it. Fly, Corvus', 1500);
  assert.equal(trace.delegated('g', 1600).sent, 'after it.');
});

test('ending voice does not cancel a pending shared request', () => {
  const trace = new ConversationTrace();
  trace.shared('share_pending', 'image');
  trace.heard('stop voice');
  trace.delegated('spoken_pending');
  trace.slept();
  assert.equal(trace.entries[0].reply, '');
  assert.equal(trace.entries[2].reply, 'cancelled');
});

test('timeline preserves every heard and spoken fragment around delegation', () => {
  const trace = new ConversationTrace();
  trace.heard('check the ', 100);
  trace.spoke('One moment.');
  trace.heard('deployment', 200);
  const delegation = trace.delegated('item_1', 500);
  trace.hub('item_1', ['running'], 870);
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
  trace.hub('item_overlap', ['running'], 10);
  trace.spoke('It is ', 100, 200);
  trace.heard('sorry', 0, 150);
  trace.spoke('running.', 200, 300);
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'spoken').map((entry) => entry.source), ['model after hub reply', 'model after hub reply']);
});

test('speech after a later user turn is tagged model alone', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_later');
  trace.hub('item_later', ['running'], 10);
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
  trace.hub('item_3', ['deployed'], 50);
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
  trace.slept();
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'delegation').map(({ reply, failed }) => ({ reply, failed })), [
    { reply: 'hub reply timed out', failed: true },
    { reply: 'cancelled', failed: true },
  ]);
});

test('delegation context preserves transcript text exactly', () => {
  const trace = new ConversationTrace();
  trace.heard('what about [this]');
  trace.delegated('d0');
  trace.spoke('Yes [nod], the [thumbs up] build is green [shru');
  trace.heard('and now');
  const entry = trace.delegated('d1');
  assert.deepEqual(entry.context, [{ speaker: 'user', text: 'what about [this]' }, { speaker: 'live', text: 'Yes [nod], the [thumbs up] build is green [shru' }]);
});

test('a woken session carries each earlier conversation and how long ago it went to sleep', () => {
  const trace = new ConversationTrace();
  assert.deepEqual(trace.wake(), {});
  trace.heard('Remember the blue lantern.');
  trace.spoke('The lantern is blue.');
  trace.slept(0);
  trace.spoke('Hello again.');
  trace.heard('status');
  trace.delegated('item_status');
  trace.slept(170000);
  const { context, wake } = trace.wake(200000);
  assert.equal(context.length, 1);
  assert.equal(context[0].speaker, 'user');
  assert.match(context[0].text, /^Context: Archived transcripts of completed conversations, oldest first, for memory only\. Each ended when you went to sleep\./);
  assert.deepEqual(JSON.parse(context[0].text.slice(context[0].text.indexOf('\n') + 1)), [
    { turns: [{ speaker: 'user', text: 'Remember the blue lantern.' }, { speaker: 'live', text: 'The lantern is blue.' }], went_to_sleep: 'about 3 minutes ago' },
    { turns: [{ speaker: 'live', text: 'Hello again.' }, { speaker: 'user', text: 'status' }], went_to_sleep: 'about 30 seconds ago' },
  ]);
  assert.match(wake, /^The previous conversation ended and you went to sleep about 30 seconds ago\. You have just been woken for a new conversation\./);
  assert.deepEqual([171000, 170000 + 5 * 3600000, 170000 + 3 * 86400000].map((now) => /about (.+) ago\. You/.exec(trace.wake(now).wake)[1]), ['1 second', '5 hours', '3 days']);
});

test('woken-session memory stays within the wire limit by dropping the oldest turns first', () => {
  const trace = new ConversationTrace();
  for (let session = 0; session < 3; session++) {
    for (let turn = 0; turn < 10; turn++) {
      trace.heard(`${session}.${turn}:` + '"\\雪'.repeat(110));
      trace.spoke('ok');
    }
    trace.slept(session);
  }
  const { context } = trace.wake(10);
  assert.ok(new TextEncoder().encode(JSON.stringify(context)).length <= 8192);
  const turns = JSON.parse(context[0].text.slice(context[0].text.indexOf('\n') + 1)).flatMap((conversation) => conversation.turns);
  assert.ok(turns.length > 1 && turns.length < 60);
  assert.ok(turns.at(-2).text.startsWith('2.9:'));
  assert.equal(turns.at(-1).text, 'ok');
  assert.equal(trace.entries.length, 60);
});

const content = (text) => [{ type: 'input_text', text }];

test('session context appends on the speakable, commentary and developer channels', () => {
  for (const channel of ['speakable', 'commentary', 'developer']) assert.deepEqual(contextEvent(channel, 'Hello.'), { type: 'session.context.append', channel, content: content('Hello.') });
  assert.throws(() => contextEvent('instructions', 'Hello.'), /refused Live client event "session.context.append"/);
});

test('a delegation context append is speakable and names its delegation item', () => {
  assert.deepEqual(contextEvent('speakable', 'Four jobs.', 'item_1'), { type: 'delegation.context.append', delegation_item_id: 'item_1', channel: 'speakable', content: content('Four jobs.') });
  for (const channel of ['commentary', 'developer']) assert.throws(() => contextEvent(channel, 'Four jobs.', 'item_1'), /refused/);
  assert.throws(() => contextEvent('speakable', 'Four jobs.', ''), /refused/);
  assert.throws(() => checkedClientEvent({ type: 'delegation.context.append', delegation_item_id: 7, channel: 'speakable', content: content('Four jobs.') }), /refused/);
});

test('one invalid client event would close the call, so the allowlist refuses it before it is sent', () => {
  const valid = { type: 'session.context.append', channel: 'speakable', content: content('Hello.') };
  assert.equal(checkedClientEvent(valid), valid);
  for (const type of ['session.instructions.append', 'session.commentary.append', 'response.create', 'session.update']) assert.throws(() => checkedClientEvent({ ...valid, type }), new RegExp(`refused Live client event "${type}"`));
  for (const event of [
    null,
    'session.context.append',
    { ...valid, event_id: 'hub_1' },
    { type: 'delegation.context.append', delegation_item_id: 'item_1', channel: 'speakable', content: content('Hello.'), delegation_id: 'item_1' },
    { type: 'session.context.append', channel: 'speakable' },
    { ...valid, content: content('') },
    { ...valid, content: content('   ') },
    { ...valid, content: 'Hello.' },
    { ...valid, content: [] },
    { ...valid, content: [...content('Hello.'), ...content('Again.')] },
    { ...valid, content: [{ type: 'output_text', text: 'Hello.' }] },
    { ...valid, content: [{ type: 'input_text', text: 'Hello.', extra: true }] },
    { ...valid, content: [{ type: 'input_text', text: 7 }] },
  ]) assert.throws(() => checkedClientEvent(event), /refused Live client event/, JSON.stringify(event));
  assert.throws(() => contextEvent('speakable', ''), /refused/);
});

test('a usage limit is reported only when the server sets one', () => {
  for (const event of [{}, { usage_limit: null }, { usage_limit: {} }, { usage_limit: { status: null, reset_seconds: null } }, null]) assert.equal(usageLimit(event), null);
  assert.deepEqual(usageLimit({ usage: { audio_duration_ms: 1 }, usage_limit: { status: 'exhausted', reset_seconds: 120, extra: 1 } }), { status: 'exhausted', reset_seconds: 120 });
  assert.deepEqual(usageLimit({ usage_limit: { reset_seconds: 0 } }), { status: null, reset_seconds: 0 });
});
