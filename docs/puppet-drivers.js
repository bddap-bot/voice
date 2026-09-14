const RULES = [
  ['apologetic', /\b(?:sorry|apolog(?:y|ize|ise|etic)|regret|my fault|forgive)\b/i],
  ['surprised', /\b(?:wow|whoa|unexpected|astonish|surpris|suddenly|incredible)\w*\b/i],
  ['amused', /\b(?:haha|hehe|funny|hilarious|joke|laugh|delightful)\b/i],
  ['pleased', /\b(?:great|excellent|wonderful|glad|happy|perfect|success|done)\b/i],
  ['puzzled', /\b(?:confus|puzzl|unclear|don't understand|doesn't make sense|odd)\w*\b/i],
  ['skeptical', /\b(?:doubt|skeptic|unlikely|not convinced|questionable|supposedly)\w*\b/i],
  ['thinking', /\b(?:think|consider|reason|perhaps|maybe|let me see|work through)\w*\b/i],
  ['alert', /\b(?:warning|careful|urgent|attention|important|danger|watch out)\b/i],
  ['sleepy', /\b(?:sleep|tired|drowsy|exhausted|rest|good night)\w*\b/i],
  ['curious', /(?:\?|\b(?:curious|wonder|interesting|tell me|how|why|what if)\b)/i],
];

export function transcriptMood(text) {
  return transcriptMoodMatch(text)?.mood ?? null;
}

function transcriptMoodMatch(text) {
  let latest = null;
  let latestIndex = -1;
  for (const [mood, pattern] of RULES) {
    const matches = text.matchAll(new RegExp(pattern.source, `${pattern.flags}g`));
    for (const match of matches) if (match.index > latestIndex) {
      latest = { mood, index: match.index };
      latestIndex = match.index;
    }
  }
  return latest;
}

export class TranscriptMoodDriver {
  constructor(apply) {
    this.apply = apply;
    this.text = '';
    this.bufferStart = 0;
    this.lastCue = -1;
  }
  push(delta) {
    this.text += delta;
    if (this.text.length > 320) {
      const removed = this.text.length - 320;
      this.text = this.text.slice(removed);
      this.bufferStart += removed;
    }
    const match = transcriptMoodMatch(this.text);
    const cue = match ? this.bufferStart + match.index : -1;
    if (match && cue > this.lastCue) {
      this.lastCue = cue;
      this.apply(match.mood);
    }
    return match?.mood ?? null;
  }
  reset() {
    this.text = '';
    this.bufferStart = 0;
    this.lastCue = -1;
  }
}
