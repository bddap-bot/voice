const LABELS = {
  none: {
    neutral: ['the available colors are red, green, and blue', 'the choices are small, medium, and large', 'the expressions include happy, sad, angry, and surprised', 'the list includes alpha, beta, gamma, and delta', 'first is setup, second is execution, and third is review'],
  },
  gesture: {
    nod: ['yes, I agree completely', 'that conclusion is correct', 'absolutely, you have my approval', 'I concur with that', 'please proceed with the plan'],
    shrug: ['I do not know', 'it could be either way', 'I have no preference'],
    think: ['let me reason about this', 'I need to consider the options', 'give me a moment to work it out'],
    point: ['look at this part', 'notice the item over there', 'here is the important detail'],
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

let webGpuAdapterPromise;

export function webGpuAdapter() {
  webGpuAdapterPromise ??= Promise.resolve().then(() => navigator.gpu?.requestAdapter()).catch(() => null).then((adapter) => {
    if (!adapter) console.info('WebGPU adapter unavailable; action embeddings use the WebAssembly CPU fallback.');
    return adapter ?? null;
  });
  return webGpuAdapterPromise;
}

export async function browserEmbedder() {
  const { env, pipeline } = await import('https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.7.3');
  env.allowLocalModels = false;
  const adapter = await webGpuAdapter();
  const extractor = await pipeline('feature-extraction', 'Xenova/all-MiniLM-L6-v2', { dtype: 'q8', device: adapter ? 'webgpu' : 'wasm' });
  return async (texts) => (await extractor(texts, { pooling: 'mean', normalize: true })).tolist();
}

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
    if (/\b(?:sit(?:ting)?|stand(?:ing)?|back up)\b/i.test(text)) return { kind: 'none', name: 'neutral', score: 1 };
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

export class TranscriptActionDriver {
  constructor(apply, classifier = new EmbeddingActionClassifier(), timing = {}) {
    this.apply = apply;
    this.classifier = classifier;
    this.now = timing.now ?? (() => performance.now());
    this.schedule = timing.schedule ?? ((apply, delay) => setTimeout(apply, delay));
    this.minimumMs = timing.minimumMs ?? 900;
    this.epoch = 0;
    this.pending = '';
    this.nextAt = 0;
    this.tail = Promise.resolve();
  }
  push(delta, playAt = this.now()) {
    this.pending += delta;
    const { sentences, pending } = completedSentences(this.pending);
    this.pending = pending;
    for (const sentence of sentences) this.sentence(sentence, playAt);
  }
  flush() {
    this.sentence(this.pending);
    this.pending = '';
  }
  sentence(raw, playAt = this.now()) {
    if (raw.replace(/[.!?\s]/g, '')) this.dispatch(raw.trim(), playAt);
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
    this.nextAt = 0;
  }
}

export class ListeningReactionDriver {
  constructor(apply, timing = {}) {
    this.apply = apply;
    this.now = timing.now ?? (() => performance.now());
    this.schedule = timing.schedule ?? ((apply) => setTimeout(apply, 50));
    this.active = false;
    this.startedAt = 0;
    this.stoppedAt = 0;
    this.scheduled = false;
  }
  activity(active) {
    const now = this.now();
    if (active) {
      if (this.active) return;
      this.active = true;
      this.startedAt = now;
      this.stoppedAt = 0;
    } else {
      if (!this.active) return;
      this.active = false;
      this.stoppedAt = now;
    }
    if (!this.scheduled) this.tick();
  }
  tick() {
    this.scheduled = false;
    const now = this.now();
    const attack = Math.min(1, Math.max(0, (now - this.startedAt) / 240));
    const release = this.active ? 1 : Math.min(1, Math.max(0, (this.stoppedAt + 420 - now) / 300));
    const amount = attack * release;
    const elapsed = Math.max(0, now - this.startedAt);
    const nod = this.active && elapsed > 900 ? Math.sin((elapsed - 900) * Math.PI / 520) * 0.035 * amount : 0;
    const tilt = this.active && elapsed > 2100 ? Math.sin((elapsed - 2100) * Math.PI / 1700) * 0.055 * amount : 0;
    this.apply({ amount, lean: amount ? -0.055 * amount : 0, nod, tilt });
    if ((this.active || amount > 0) && !this.scheduled) {
      this.scheduled = true;
      this.schedule(() => this.tick());
    }
  }
  reset() {
    this.active = false;
    this.startedAt = 0;
    this.stoppedAt = 0;
    this.apply({ amount: 0, lean: 0, nod: 0, tilt: 0 });
  }
}

export { LABELS };
