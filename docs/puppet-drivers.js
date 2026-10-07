const LABELS = {
  none: {
    neutral: ['the available colors are red, green, and blue', 'the choices are small, medium, and large', 'the expressions include happy, sad, angry, and surprised', 'the list includes alpha, beta, gamma, and delta', 'first is setup, second is execution, and third is review'],
  },
  gesture: {
    nod: ['yes, I agree completely', 'that conclusion is correct', 'absolutely, you have my approval', 'I concur with that', 'please proceed with the plan'],
    shrug: ['I do not know', 'it could be either way', 'I have no preference'],
    think: ['let me reason about this', 'I need to consider the options', 'give me a moment to work it out'],
    wave: ['hello, good to see you', 'goodbye, see you later', 'welcome'],
    no: ['no, I disagree with that', 'that is not correct', 'I have to reject that idea'],
    laugh: ['that made me burst out laughing', 'I cannot stop laughing at that', 'I laughed out loud when I heard that'],
    clap: ['that deserves a round of applause', 'let us applaud that achievement', 'I am clapping for that performance'],
    bow: ['I am honored to meet you', 'thank you with my deepest respect', 'please accept my respectful greeting'],
    'thumbs-up': ['that gets a big thumbs up from me', 'what an emphatic sign of success', 'this result earns the strongest endorsement'],
    stretch: ['I need to stretch after sitting so long', 'let me loosen up my stiff shoulders', 'time to take a quick stretch break'],
    'look-around': ['let me look around the room', 'I am checking what is around us', 'let me survey our surroundings'],
  },
  mood: {
    neutral: ['I feel neutral', 'I have no particular emotion', 'my expression is neutral'],
    apologetic: ['I am sorry', 'please forgive my mistake', 'that was my fault'],
    surprised: ['that is completely unexpected', 'what an astonishing result', 'I cannot believe it'],
    amused: ['that is hilarious', 'what a funny joke', 'this makes me laugh'],
    pleased: ['I am delighted with the result', 'excellent work', 'this turned out wonderfully', 'I feel happy today'],
    sad: ['I feel sad about what happened', 'this news has left me unhappy', 'I am feeling down today'],
    angry: ['I am angry about what happened', 'this situation makes me furious', 'I feel upset and mad'],
    puzzled: ['I do not understand this', 'this is confusing', 'that does not make sense'],
    skeptical: ['I am not convinced', 'that claim seems doubtful', 'I question whether that is true'],
    thinking: ['I am considering the problem', 'let me think this through', 'perhaps there is another approach'],
    alert: ['please be careful', 'this is an urgent warning', 'watch out for danger'],
    sleepy: ['I am exhausted and need sleep', 'I feel drowsy', 'it is time to rest'],
    relaxed: ['I feel relaxed and at ease', 'everything feels calm and peaceful', 'I can finally unwind'],
    curious: ['I wonder how that works', 'why did this happen', 'I would like to learn more'],
  },
};

const KEYWORDS = {
  apologetic: /sorry|fault|forgive/, surprised: /wow|unexpected|astonish/, amused: /funny|joke|laugh/,
  pleased: /great|excellent|wonderful/, puzzled: /confus|understand|sense/, skeptical: /doubt|convinced|question/,
  thinking: /think|consider|perhaps/, alert: /warning|careful|danger/, sleepy: /sleep|tired|drowsy/, curious: /wonder|why|how/,
};

function cosine(a, b) {
  let dot = 0;
  let aa = 0;
  let bb = 0;
  for (let i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    aa += a[i] * a[i];
    bb += b[i] * b[i];
  }
  return dot / Math.sqrt(aa * bb);
}

function mean(rows) {
  return rows[0].map((_, i) => rows.reduce((sum, row) => sum + row[i], 0) / rows.length);
}

export function keywordMood(text) {
  for (const [name, pattern] of Object.entries(KEYWORDS)) if (pattern.test(text.toLowerCase())) return { kind: 'mood', name };
  return null;
}

export const EMBEDDING_MODEL = 'Xenova/all-MiniLM-L6-v2';

let webGpuAdapterPromise;

export function webGpuAdapter() {
  webGpuAdapterPromise ??= Promise.resolve().then(() => navigator.gpu?.requestAdapter()).catch(() => null).then((adapter) => {
    if (!adapter) console.info('WebGPU adapter unavailable; action embeddings use the WebAssembly CPU fallback.');
    return adapter ?? null;
  });
  return webGpuAdapterPromise;
}

export function independentEmbedder(extractor) {
  return async (texts) => {
    const vectors = [];
    // q8 inference can depend on batch padding and quantization ranges.
    // Keep each inference independent of the other texts and label count.
    for (const text of texts) {
      const [vector] = (await extractor([text], { pooling: 'mean', normalize: true })).tolist();
      vectors.push(vector);
    }
    return vectors;
  };
}

export async function browserEmbedder() {
  const { env, pipeline } = await import('./lib/transformers.js');
  env.allowLocalModels = false;
  const adapter = await webGpuAdapter();
  const extractor = await pipeline('feature-extraction', EMBEDDING_MODEL, { dtype: 'q8', device: adapter ? 'webgpu' : 'wasm' });
  return independentEmbedder(extractor);
}

export const POSTURE = /\b(?:sit(?:ting)?|stand(?:ing)?|back up)\b/i;

export class EmbeddingActionClassifier {
  constructor(loadEmbedder = globalThis.__voiceLoadEmbedder ?? browserEmbedder) {
    this.ready = this.initialize(loadEmbedder);
  }
  async initialize(loadEmbedder) {
    this.embed = await loadEmbedder();
    const entries = Object.entries(LABELS).flatMap(([kind, labels]) => Object.entries(labels).map(([name, examples]) => ({ kind, name, examples })));
    const vectors = await this.embed(entries.flatMap(({ examples }) => examples));
    let offset = 0;
    this.centroids = entries.map(({ kind, name, examples }) => ({ kind, name, centroid: mean(vectors.slice(offset, offset += examples.length)) }));
  }
  async classify(text) {
    await this.ready;
    if (POSTURE.test(text)) return { kind: 'none', name: 'neutral', score: 1 };
    const [vector] = await this.embed([text]);
    return this.centroids.reduce((best, candidate) => {
      const score = cosine(vector, candidate.centroid);
      return !best || score > best.score ? { kind: candidate.kind, name: candidate.name, score } : best;
    }, null);
  }
}

function completedSentences(text) {
  const sentences = [];
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    if (/[.!?]/.test(text[i])) {
      while (/[.!?]/.test(text[i + 1] ?? '')) i++;
      sentences.push(text.slice(start, i + 1));
      start = i + 1;
    }
  }
  return { sentences, pending: text.slice(start) };
}

const ACTION_KINDS = new Map(['gesture', 'mood'].flatMap((kind) => Object.keys(LABELS[kind]).map((name) => [name, kind])));
const BRACKETED = /\s*\[([^[\]]*)\]/g;
const LONGEST_TOKEN = 40;
const bracketed = (kind) => Object.keys(LABELS[kind]).map((name) => `[${name}]`).join(', ');

export const ACTION_INSTRUCTIONS = `Your body is an animated figure. When you make a gesture or show a mood, write its token in square brackets at the point in the sentence where it happens, for example "[nod] Yes, that works." Gestures: ${bracketed('gesture')}. Moods: ${bracketed('mood')}. Use only these tokens, and only where the action fits. The page acts them out and never shows them; speak everything else as usual.`;

export class TranscriptActionDriver {
  constructor(apply, classifier = new EmbeddingActionClassifier(), timing = {}, compare = () => {}) {
    this.apply = apply;
    this.classifier = classifier;
    this.compare = compare;
    this.now = timing.now ?? (() => performance.now());
    this.schedule = timing.schedule ?? ((apply, delay) => setTimeout(apply, delay));
    this.minimumMs = timing.minimumMs ?? 900;
    this.epoch = 0;
    this.pending = '';
    this.held = '';
    this.annotations = [];
    this.nextAt = 0;
    this.tail = Promise.resolve();
  }
  push(delta, playAt = this.now()) {
    let text = this.held + delta;
    const open = text.lastIndexOf('[');
    this.held = open >= 0 && !text.includes(']', open) && text.length - open <= LONGEST_TOKEN ? text.slice(open) : '';
    text = text.slice(0, text.length - this.held.length);
    let spoken = '';
    let last = 0;
    for (const match of text.matchAll(BRACKETED)) {
      spoken += this.speak(text.slice(last, match.index), playAt);
      this.annotate(match[1], playAt);
      last = match.index + match[0].length;
    }
    return spoken + this.speak(text.slice(last), playAt);
  }
  speak(text, playAt) {
    this.pending += text;
    const { sentences, pending } = completedSentences(this.pending);
    this.pending = pending;
    for (const sentence of sentences) this.sentence(sentence, playAt);
    return text;
  }
  annotate(raw, playAt) {
    const name = raw.trim().toLowerCase().replace(/\s+/g, '-');
    const kind = ACTION_KINDS.get(name);
    if (!kind) return this.apply({ kind: 'unknown', name, source: 'bracket' }, { token: raw, playAtMinusNow: 0 });
    this.annotations.push({ kind, name });
    return this.run(async () => ({ kind, name, source: 'bracket' }), playAt, { token: raw });
  }
  sentence(raw, playAt = this.now()) {
    const annotations = this.annotations;
    this.annotations = [];
    if (!raw.replace(/[.!?\s]/g, '')) return;
    const text = raw.trim();
    if (!annotations.length) return this.dispatch(text, playAt);
    const epoch = this.epoch;
    return this.classifier.classify(text).then((choice) => {
      if (epoch === this.epoch) this.compare({ sentence: text, annotations, classifier: choice });
    }, () => {});
  }
  endTurn() {
    const text = this.pending;
    this.pending = '';
    this.held = '';
    if (this.annotations.length && text.trim()) return this.sentence(text);
    this.annotations = [];
  }
  dispatch(text, playAt = this.now()) {
    return this.run(async () => ({ ...await this.classifier.classify(text), source: 'classifier' }), playAt, { sentence: text });
  }
  run(produce, playAt, detail) {
    const epoch = this.epoch;
    const operation = this.tail.then(async () => {
      const result = await produce();
      if (epoch !== this.epoch) return result;
      const at = Math.max(playAt, this.nextAt, this.now());
      this.nextAt = at + this.minimumMs;
      const playAtMinusNow = Math.max(0, at - this.now());
      await new Promise((resolve) => this.schedule(resolve, playAtMinusNow));
      if (epoch === this.epoch) this.apply(result, { ...detail, playAtMinusNow });
      return result;
    });
    this.tail = operation.catch(() => {});
    return operation;
  }
  reset() {
    this.epoch++;
    this.pending = '';
    this.held = '';
    this.annotations = [];
    this.nextAt = 0;
  }
}

export { LABELS };
