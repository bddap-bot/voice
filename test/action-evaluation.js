import assert from 'node:assert/strict';
import { EmbeddingActionClassifier, LABELS, keywordMood } from '../docs/puppet-drivers.js';

export const evaluation = [
  ['none','neutral','The available expressions are neutral, happy, sad, angry, surprised, blink, and mouth open.'], ['none','neutral','The choices are small, medium, and large.'], ['none','neutral','The colors are red, green, and blue.'],
  ['gesture','nod','I concur with that conclusion.'], ['gesture','nod','You have my approval.'], ['gesture','nod','That answer checks out.'], ['gesture','nod','Proceed with the plan.'],
  ['gesture','shrug','Your guess is as good as mine.'], ['gesture','shrug','Either option works for me.'], ['gesture','shrug','I have no idea.'],
  ['gesture','think','I need a moment to weigh the alternatives.'], ['gesture','think','Let me reason through the consequences.'], ['gesture','think','I will work out the answer.'],
  ['gesture','point','Focus on the chart to the right.'], ['gesture','point','Take a look at the highlighted section.'], ['gesture','point','The key evidence is over there.'],
  ['gesture','wave','It is lovely to meet you.'], ['gesture','wave','Farewell until next time.'], ['gesture','wave','Hi there, welcome in.'],
  ['gesture','no','No, I cannot agree with that.'], ['gesture','no','That conclusion is simply incorrect.'], ['gesture','no','I reject that suggestion.'],
  ['gesture','laugh','That made me laugh out loud.'], ['gesture','laugh','I burst out laughing at the punchline.'], ['gesture','laugh','I am still laughing about that story.'],
  ['gesture','clap','That performance deserves a round of applause.'], ['gesture','clap','Let us applaud the team for their work.'], ['gesture','clap','I am clapping for that wonderful achievement.'],
  ['gesture','bow','It is an honor to meet you.'], ['gesture','bow','Please accept my deepest respect.'], ['gesture','bow','Thank you for this gracious welcome.'],
  ['gesture','thumbs-up','This plan has my enthusiastic approval.'], ['gesture','thumbs-up','I give that result a big thumbs up.'], ['gesture','thumbs-up','You have my strongest endorsement.'],
  ['gesture','stretch','I need to stretch my stiff shoulders.'], ['gesture','stretch','Let me loosen up after sitting so long.'], ['gesture','stretch','It is time for a quick stretch break.'],
  ['gesture','look-around','Let me look around the room.'], ['gesture','look-around','I am checking our surroundings.'], ['gesture','look-around','Let me survey what is around us.'],
  ['mood','apologetic','I owe you an apology.'], ['mood','apologetic','Please accept my sincere remorse.'], ['mood','apologetic','I take responsibility for the error.'],
  ['mood','surprised','That outcome caught me completely off guard.'], ['mood','surprised','What an astonishing turn of events.'], ['mood','surprised','I never saw that coming.'],
  ['mood','amused','That story made me chuckle.'], ['mood','amused','The punchline was hilarious.'], ['mood','amused','I cannot stop giggling at that.'],
  ['mood','pleased','I am delighted by how well this went.'], ['mood','pleased','The result is deeply satisfying.'], ['mood','pleased','Everything turned out beautifully.'],
  ['mood','sad','I feel sad about the news.'], ['mood','sad','This outcome has left me unhappy.'], ['mood','sad','I am feeling down today.'],
  ['mood','angry','I am angry about this decision.'], ['mood','angry','That behavior makes me furious.'], ['mood','angry','I feel upset and mad.'],
  ['mood','puzzled','I cannot make heads or tails of this.'], ['mood','puzzled','This explanation leaves me baffled.'], ['mood','puzzled','Something here does not add up.'],
  ['mood','skeptical','That assertion is hard to believe.'], ['mood','skeptical','I remain unconvinced by the evidence.'], ['mood','skeptical','The claim sounds rather dubious.'],
  ['mood','thinking','I am contemplating a different solution.'], ['mood','thinking','There may be another angle to explore.'], ['mood','thinking','I am reflecting on the implications.'],
  ['mood','alert','Mind the hazard ahead.'], ['mood','alert','This situation demands immediate attention.'], ['mood','alert','Stay vigilant around that edge.'],
  ['mood','sleepy','I can barely keep my eyes open.'], ['mood','sleepy','A nap sounds irresistible right now.'], ['mood','sleepy','My energy is fading fast.'],
  ['mood','relaxed','I feel relaxed and at ease.'], ['mood','relaxed','Everything feels calm and peaceful.'], ['mood','relaxed','I can finally unwind.'],
  ['mood','curious','What causes this behavior?'], ['mood','curious','I would love to discover what comes next.'], ['mood','curious','Tell me more about the mechanism.'],
  ['pose','sit','I am sitting down now.'], ['pose','sit','Let me take a seat.'], ['pose','sit','I will sit in this chair.'],
  ['pose','stand','I am standing up now.'], ['pose','stand','Let me get to my feet.'], ['pose','stand','I will stand here.'],
  ['gesture','nod','I am fully on board.'], ['gesture','shrug','It makes no difference to me.'], ['mood','pleased','I could not be happier with it.'], ['mood','puzzled','This has me completely stumped.'],
];

export async function evaluate(loadEmbedder) {
  const classifier = new EmbeddingActionClassifier(loadEmbedder);
  let before = 0;
  let after = 0;
  const rows = new Map(Object.entries(LABELS).flatMap(([kind, names]) => Object.keys(names).map((name) => [`${kind}:${name}`, { token: `${kind}:${name}`, examples: [], hits: 0 }])));
  for (const [kind, name, text] of evaluation) {
    const baseline = keywordMood(text);
    if (baseline?.kind === kind && baseline?.name === name) before++;
    const result = await classifier.classify(text);
    const hit = result.kind === kind && result.name === name;
    if (hit) after++;
    const row = rows.get(`${kind}:${name}`);
    row.examples.push({ prior: ['We are reviewing the result.', 'Please continue.'], sentence: text, classified: `${result.kind}:${result.name}`, hit });
    if (hit) row.hits++;
  }
  for (const row of rows.values()) assert.ok(row.hits, `unreachable action token: ${row.token}`);
  for (const [sentence, token] of [['Yes.', 'gesture:nod'], ['No idea.', 'gesture:shrug'], ['Hmm.', 'mood:thinking'], ['Happy.', 'mood:pleased'], ['Sad.', 'mood:sad'], ['Angry.', 'mood:angry'], ['Relaxed.', 'mood:relaxed'], ['Surprised.', 'mood:surprised'], ['Pointing at the panel.', 'gesture:point'], ['Sitting.', 'pose:sit'], ['And standing.', 'pose:stand']]) {
    const result = await classifier.classify(sentence);
    assert.equal(`${result.kind}:${result.name}`, token, `short sentence ${sentence}`);
  }
  for (const sentence of ['The available expressions are neutral, happy, sad, angry, surprised, blink, and mouth open.', 'The list includes alpha, beta, gamma, and delta.', 'First is setup, second is execution, and third is review.']) {
    const result = await classifier.classify(sentence);
    assert.notEqual(`${result.kind}:${result.name}`, 'gesture:point', `enumeration sentence ${sentence}`);
  }
  return { total: evaluation.length, before, after, rows: [...rows.values()].map((row) => ({ ...row, hitRate: `${row.hits}/${row.examples.length}` })) };
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const { pipeline } = await import('@huggingface/transformers');
  const extractor = await pipeline('feature-extraction', 'Xenova/all-MiniLM-L6-v2', { dtype: 'q8' });
  const result = await evaluate(async () => async (texts) => (await extractor(texts, { pooling: 'mean', normalize: true })).tolist());
  console.table(result.rows.map(({ token, hitRate }) => ({ token, hitRate })));
  console.log(JSON.stringify({ total: result.total, before: result.before, after: result.after }));
}
