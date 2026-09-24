import assert from 'node:assert/strict';
import { pipeline } from '@huggingface/transformers';
import { WAKE_PHRASE } from '../docs/identity.js';
import { SpeechSegmenter, WAKE_MODEL, WAKE_SCORE, wakeScore } from '../docs/wake.js';
import { ordinarySpeech, samples16k, synthesize, voices } from './speech.js';

const recognize = await pipeline('automatic-speech-recognition', WAKE_MODEL.id, { dtype: WAKE_MODEL.dtype });

async function hear(text, voice) {
  const audio = samples16k(await synthesize(text, voice));
  const padded = new Float32Array(audio.length + 3 * 16000);
  padded.set(audio, 16000);
  const heard = [];
  for (const segment of new SpeechSegmenter(16000).push(padded)) heard.push((await recognize(segment)).text.trim());
  const scores = heard.map((item) => wakeScore(item, WAKE_PHRASE).score);
  return { spoken: text, voice: `${voice.voice} ${voice.speed}`, heard: heard.join(' | '), score: Math.max(0, ...scores) };
}

const phrase = [];
const ordinary = [];
for (const voice of voices) {
  phrase.push(await hear(WAKE_PHRASE, voice));
  for (const text of ordinarySpeech) ordinary.push(await hear(text, voice));
}
console.table([...phrase, ...ordinary].map((row) => ({ ...row, score: row.score.toFixed(2), wakes: row.score >= WAKE_SCORE })));
for (const row of phrase) assert.ok(row.score >= WAKE_SCORE, `the spotter missed ${JSON.stringify(row)}`);
for (const row of ordinary) assert.ok(row.score < WAKE_SCORE, `ordinary speech woke the spotter: ${JSON.stringify(row)}`);
console.log(JSON.stringify({ phrase: WAKE_PHRASE, model: WAKE_MODEL, voices: voices.length, woke: phrase.length, ordinary: ordinary.length, falseWakes: 0 }));
