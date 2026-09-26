import assert from 'node:assert/strict';
import test from 'node:test';
import { modelToolFields } from '../docs/live.js';

test('session telemetry allowlists model and tool fields without private text', () => {
  const privateFields = { instructions: 'private instructions', commentary: 'private commentary', situation: 'private state', transcript: 'private transcript', input: [{ content: 'private history' }], unknown: { model: 'private nested text' } };
  const session = {
    ...privateFields,
    model: 'live-model',
    tools: [{ type: 'function', name: 'sleep', description: 'private description', parameters: privateFields }],
    delegation: {
      ...privateFields, type: 'responses', target: 'responses', model: 'delegate-model',
      responses: { ...privateFields, model: 'responses-model', tools: [{ name: 'hub', ...privateFields }, { type: 'web_search' }, { function: { name: 'perform', ...privateFields } }] },
    },
  };
  assert.deepEqual(modelToolFields({ ...privateFields, session }), { session: {
    model: 'live-model', tools: ['sleep'],
    delegation: { type: 'responses', target: 'responses', model: 'delegate-model', responses: { model: 'responses-model', tools: ['hub', 'web_search', 'perform'] } },
  } });
  assert.deepEqual(modelToolFields({ model: 'live-model', instructions: 'secret' }), { model: 'live-model' });
});

test('missing models remain absent and SDP, arguments and output are excluded', () => {
  assert.deepEqual(modelToolFields({ sdp: 'private SDP', session_id: 'id', transport: { sdp: 'private SDP' } }), {});
  assert.deepEqual(modelToolFields({ delegation: { target: 'responses', instructions: 'secret' } }), { delegation: { target: 'responses' } });
  assert.deepEqual(modelToolFields({ response: { model: 'reported-model', output: [{ text: 'private' }] } }), { response: { model: 'reported-model' } });
  assert.deepEqual(modelToolFields({ item: { type: 'function_call', name: 'hub', arguments: 'private arguments' } }), { tools: ['hub'] });
  for (const input of [null, undefined, [], 'private', { model: {}, tools: [null, { name: {} }, { description: 'private' }], delegation: { type: {}, target: {} } }]) {
    assert.doesNotMatch(JSON.stringify(modelToolFields(input)), /private/);
  }
});
