import { EmbeddingActionClassifier, keywordMood } from '../docs/puppet-drivers.js';

export const evaluation = [
  ['gesture','nod','I concur with that conclusion.'], ['gesture','nod','You have my approval.'], ['gesture','nod','That answer checks out.'], ['gesture','nod','Proceed with the plan.'],
  ['gesture','shrug','Your guess is as good as mine.'], ['gesture','shrug','Either option works for me.'], ['gesture','shrug','I have no idea.'],
  ['gesture','think','I need a moment to weigh the alternatives.'], ['gesture','think','Let me reason through the consequences.'], ['gesture','think','I will work out the answer.'],
  ['gesture','point','Focus on the chart to the right.'], ['gesture','point','Take a look at the highlighted section.'], ['gesture','point','The key evidence is over there.'],
  ['gesture','wave','It is lovely to meet you.'], ['gesture','wave','Farewell until next time.'], ['gesture','wave','Hi there, welcome in.'],
  ['mood','apologetic','I owe you an apology.'], ['mood','apologetic','Please accept my sincere remorse.'], ['mood','apologetic','I take responsibility for the error.'],
  ['mood','surprised','That outcome caught me completely off guard.'], ['mood','surprised','What an astonishing turn of events.'], ['mood','surprised','I never saw that coming.'],
  ['mood','amused','That story made me chuckle.'], ['mood','amused','The punchline was hilarious.'], ['mood','amused','I cannot stop giggling at that.'],
  ['mood','pleased','I am delighted by how well this went.'], ['mood','pleased','The result is deeply satisfying.'], ['mood','pleased','Everything turned out beautifully.'],
  ['mood','puzzled','I cannot make heads or tails of this.'], ['mood','puzzled','This explanation leaves me baffled.'], ['mood','puzzled','Something here does not add up.'],
  ['mood','skeptical','That assertion is hard to believe.'], ['mood','skeptical','I remain unconvinced by the evidence.'], ['mood','skeptical','The claim sounds rather dubious.'],
  ['mood','thinking','I am contemplating a different solution.'], ['mood','thinking','There may be another angle to explore.'], ['mood','thinking','I am reflecting on the implications.'],
  ['mood','alert','Mind the hazard ahead.'], ['mood','alert','This situation demands immediate attention.'], ['mood','alert','Stay vigilant around that edge.'],
  ['mood','sleepy','I can barely keep my eyes open.'], ['mood','sleepy','A nap sounds irresistible right now.'], ['mood','sleepy','My energy is fading fast.'],
  ['mood','curious','What causes this behavior?'], ['mood','curious','I would love to discover what comes next.'], ['mood','curious','Tell me more about the mechanism.'],
  ['gesture','nod','I am fully on board.'], ['gesture','shrug','It makes no difference to me.'], ['mood','pleased','I could not be happier with it.'], ['mood','puzzled','This has me completely stumped.'],
];

export async function evaluate(loadEmbedder) {
  const classifier = new EmbeddingActionClassifier(loadEmbedder);
  let before = 0;
  let after = 0;
  for (const [kind, name, text] of evaluation) {
    const baseline = keywordMood(text);
    if (baseline?.kind === kind && baseline?.name === name) before++;
    const result = await classifier.classify(text);
    if (result.kind === kind && result.name === name) after++;
  }
  return { total: evaluation.length, before, after };
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const { pipeline } = await import('@huggingface/transformers');
  const extractor = await pipeline('feature-extraction', 'Xenova/all-MiniLM-L6-v2', { dtype: 'q8' });
  const result = await evaluate(async () => async (texts) => (await extractor(texts, { pooling: 'mean', normalize: true })).tolist());
  console.log(JSON.stringify(result));
}
