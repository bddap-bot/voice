import { writeFileSync } from 'node:fs';
import { AutoTokenizer, pipeline } from '@huggingface/transformers';
import { ACTION_INSTRUCTIONS, EMBEDDING_MODEL, EmbeddingActionClassifier, LABELS, POSTURE, TranscriptActionDriver, independentEmbedder } from '../docs/puppet-drivers.js';
import { evaluation } from '../test/action-evaluation.js';

const out = new URL('../vr/golden/actions.json', import.meta.url).pathname;
const tokenizer = await AutoTokenizer.from_pretrained(EMBEDDING_MODEL);
const extractor = await pipeline('feature-extraction', EMBEDDING_MODEL, { dtype: 'q8' });
const classifier = new EmbeddingActionClassifier(async () => independentEmbedder(extractor));
const texts = [
  ...evaluation.map(([, , text]) => text),
  'Yes.', 'No idea.', 'Hmm.', 'Happy.', 'Sad.', 'Hi there!', "Here's a little wave for you.", 'Sure, asking.',
  'Let me sit down.', 'I am standing up now.', 'Back up a bit.', 'Café naïveté, señor — 42%?', '你好, friend.',
];
const cases = [];
const cosine = (a, b) => a.reduce((sum, value, i) => sum + value * b[i], 0) / Math.hypot(...a) / Math.hypot(...b);
for (const text of texts) {
  const { kind, name, score } = await classifier.classify(text);
  const [vector] = await classifier.embed([text]);
  const [, second] = classifier.centroids.map(({ centroid }) => cosine(vector, centroid)).sort((a, b) => b - a);
  cases.push({ text, ids: Array.from(tokenizer(text).input_ids.data, Number), kind, name, score: +score.toFixed(6), margin: +(kind === 'none' && score === 1 ? 1 : score - second).toFixed(6) });
}
// Live's output and input transcript deltas as the overlay received them, asked for a little wave before the overlay acted on speech; then a bracketed reply split across deltas.
const transcripts = [
  [['spoke', ' Sure,'], ['spoke', ' asking.'], ['spoke', ' Hi there'], ['spoke', '!'], ['spoke', " Here's"], ['spoke', ' a little'], ['spoke', ' wave'], ['spoke', ' for you'], ['spoke', '.'], ['heard', ' Okay']],
  [['spoke', ' [wa'], ['spoke', 've] Hello'], ['spoke', ' again.'], ['spoke', ' [shrug]'], ['spoke', ' [bogus] Maybe.'], ['heard', ' Hm'], ['spoke', ' [nod] Right'], ['heard', ' ok']],
];
const replays = [];
for (const events of transcripts) {
  const actions = [];
  const spoken = [];
  const driver = new TranscriptActionDriver((action, timing) => actions.push({ source: action.source, kind: action.kind, name: action.name, ...(timing.sentence ? { sentence: timing.sentence } : { token: timing.token }) }), classifier, { now: () => 0, minimumMs: 0, schedule: (apply) => apply() });
  for (const [speaker, delta] of events) {
    if (speaker === 'heard') driver.endTurn();
    else spoken.push(driver.push(delta));
    await driver.tail;
  }
  replays.push({ events, spoken, actions });
}
writeFileSync(out, JSON.stringify({
  about: 'docs/puppet-drivers.js on the real embedding model under onnxruntime-node, each case with its margin over the runner-up label: written by scripts/vr-action-golden.mjs, read and compared by the overlay host build',
  instructions: ACTION_INSTRUCTIONS,
  labels: Object.entries(LABELS).flatMap(([kind, labels]) => Object.entries(labels).map(([name, examples]) => ({ kind, name, examples }))),
  posture: { source: POSTURE.source, flags: POSTURE.flags },
  cases,
  replays,
}, null, 1) + '\n');
