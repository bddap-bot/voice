const STEP_MS = 1500;
const STEP_FIELDS = ['pose', 'gesture', 'target', 'look', 'mood'];

function indexFromSeed(seed, length) {
  let hash = 2166136261;
  for (const character of seed) hash = Math.imul(hash ^ character.codePointAt(0), 16777619);
  return (hash >>> 0) % length;
}

function validStep(step) {
  if (!step || typeof step !== 'object' || Array.isArray(step)) return false;
  return Object.entries(step).every(([field, value]) => STEP_FIELDS.includes(field) && typeof value === 'string');
}

export class PuppetTools {
  constructor(runtime, catalog, active, select, sleep) {
    this.runtime = runtime;
    this.catalog = catalog;
    this.active = active;
    this.select = select;
    this.sleep = sleep;
    this.epoch = 0;
  }
  async execute(name, encodedArguments, valid = () => true) {
    let args;
    try { args = JSON.parse(encodedArguments || '{}'); }
    catch { throw new Error('invalid tool arguments'); }
    if (name === 'perform') {
      if (!Array.isArray(args.steps) || !args.steps.length || args.steps.length > 32 || !args.steps.every(validStep)) throw new Error('invalid performance steps');
      const epoch = ++this.epoch;
      this.#play(args.steps[0]);
      this.#playRest(epoch, args.steps.slice(1), valid);
      return { ok: true };
    }
    if (name === 'randomize_appearance') {
      if (typeof args.seed !== 'string' || !args.seed || args.seed.length > 200) throw new Error('invalid appearance seed');
      const choices = this.catalog().map((avatar) => avatar.id).filter((id) => id !== this.active());
      if (choices.length) await this.select(choices[indexFromSeed(args.seed, choices.length)], valid);
      return { ok: true, changed: choices.length > 0 };
    }
    throw new Error(`unknown puppet tool ${name}`);
  }
  #play(step) {
    if (step.gesture) this.runtime.gesture(step.gesture, step.target);
    else if (step.pose) this.runtime.pose(step.pose);
    if (step.look) this.runtime.look(step.look);
    if (step.mood) this.runtime.mood(step.mood);
  }
  async #playRest(epoch, steps, valid) {
    for (const step of steps) {
      await this.sleep(STEP_MS);
      if (epoch !== this.epoch || !valid()) return;
      this.#play(step);
    }
  }
}
