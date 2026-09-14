function indexFromSeed(seed, length) {
  let hash = 2166136261;
  for (const character of seed) hash = Math.imul(hash ^ character.codePointAt(0), 16777619);
  return (hash >>> 0) % length;
}

export function completedToolCall(event) {
  const item = event?.type === 'response.output_item.done' ? event.item : null;
  return item?.type === 'function_call' && item.call_id && item.name ? item : null;
}

export class PuppetTools {
  constructor(runtime, catalog, active, select) {
    this.runtime = runtime;
    this.catalog = catalog;
    this.active = active;
    this.select = select;
  }
  async execute(name, encodedArguments, valid = () => true) {
    let args;
    try { args = JSON.parse(encodedArguments || '{}'); }
    catch { throw new Error('invalid tool arguments'); }
    if (name === 'pose') this.runtime.pose(args.name);
    else if (name === 'gesture') this.runtime.gesture(args.name, args.target);
    else if (name === 'look') this.runtime.look(args.direction);
    else if (name === 'mood') this.runtime.mood(args.name);
    else if (name === 'randomize_appearance') {
      if (typeof args.seed !== 'string' || !args.seed || args.seed.length > 200) throw new Error('invalid appearance seed');
      const choices = this.catalog().map((avatar) => avatar.id).filter((id) => id !== this.active());
      if (choices.length) await this.select(choices[indexFromSeed(args.seed, choices.length)], valid);
      return { ok: true, changed: choices.length > 0 };
    } else throw new Error(`unknown puppet tool ${name}`);
    return { ok: true };
  }
}
