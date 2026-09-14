const LABELS = {
  gesture: {
    nod: ['yes, I agree', 'that is correct', 'absolutely, go ahead'],
    shrug: ['I do not know', 'it could be either way', 'I have no preference'],
    think: ['let me reason about this', 'I need to consider the options', 'give me a moment to work it out'],
    point: ['look at this part', 'notice the item over there', 'here is the important detail'],
    wave: ['hello, good to see you', 'goodbye, see you later', 'welcome'],
  },
  mood: {
    apologetic: ['I am sorry', 'please forgive my mistake', 'that was my fault'],
    surprised: ['that is completely unexpected', 'what an astonishing result', 'I cannot believe it'],
    amused: ['that is hilarious', 'what a funny joke', 'this makes me laugh'],
    pleased: ['I am delighted with the result', 'excellent work', 'this turned out wonderfully'],
    puzzled: ['I do not understand this', 'this is confusing', 'that does not make sense'],
    skeptical: ['I am not convinced', 'that claim seems doubtful', 'I question whether that is true'],
    thinking: ['I am considering the problem', 'let me think this through', 'perhaps there is another approach'],
    alert: ['please be careful', 'this is an urgent warning', 'watch out for danger'],
    sleepy: ['I am exhausted and need sleep', 'I feel drowsy', 'it is time to rest'],
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

export async function browserEmbedder() {
  const { env, pipeline } = await import('https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.7.3');
  env.allowLocalModels = false;
  const options = { dtype: 'q8' };
  if ('gpu' in navigator) options.device = 'webgpu';
  let extractor;
  try {
    extractor = await pipeline('feature-extraction', 'Xenova/all-MiniLM-L6-v2', options);
  } catch {
    delete options.device;
    extractor = await pipeline('feature-extraction', 'Xenova/all-MiniLM-L6-v2', options);
  }
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
    const [vector] = await this.embed([text]);
    return this.centroids.reduce((best, candidate) => {
      const score = cosine(vector, candidate.centroid);
      return !best || score > best.score ? { kind: candidate.kind, name: candidate.name, score } : best;
    }, null);
  }
}

export class TranscriptActionDriver {
  constructor(apply, classifier = new EmbeddingActionClassifier()) {
    this.apply = apply;
    this.classifier = classifier;
    this.epoch = 0;
    this.pending = '';
  }
  push(delta) {
    this.pending += delta;
    const sentences = this.pending.match(/[^.!?]+[.!?]+/g) ?? [];
    const consumed = sentences.reduce((length, sentence) => length + sentence.length, 0);
    this.pending = this.pending.slice(consumed);
    for (const sentence of sentences) this.dispatch(sentence.trim());
  }
  flush() {
    if (this.pending.trim()) this.dispatch(this.pending);
    this.pending = '';
  }
  async dispatch(text) {
    const epoch = this.epoch;
    const result = await this.classifier.classify(text);
    if (epoch === this.epoch) this.apply(result);
    return result;
  }
  reset() {
    this.epoch++;
    this.pending = '';
  }
}

export { LABELS };
