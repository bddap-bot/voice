import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, rename, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import ort from 'onnxruntime-node';
import { CHUNK, GATE, MODEL, RATE, SpeakerEnrollment, similarity, voiceprint } from '../docs/speaker.js';

const ROUNDS = 8;

// Each recording weighs the same however long it runs, so a few long recordings of a television or a crowded
// room cannot outvote the user's many short conversations.
export function dominantVoice(recordings) {
  let print = voiceprint(recordings.filter((embeddings) => embeddings.length).map(voiceprint));
  let kept = [];
  for (let round = 0; round < ROUNDS; round++) {
    kept = recordings.map((embeddings) => embeddings.filter((embedding) => similarity(print, embedding) >= GATE.threshold));
    const voiced = kept.filter((embeddings) => embeddings.length);
    if (!voiced.length) throw new Error('no voice is shared across the recordings');
    print = voiceprint(voiced.map(voiceprint));
  }
  return { print, windows: kept.reduce((sum, embeddings) => sum + embeddings.length, 0), recordings: kept.filter((embeddings) => embeddings.length).length };
}

function decode(file) {
  return new Promise((resolve, reject) => {
    const child = spawn('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-i', file, '-ac', '1', '-ar', String(RATE), '-f', 'f32le', 'pipe:1'], { stdio: ['ignore', 'pipe', 'inherit'] });
    const chunks = [];
    child.stdout.on('data', (chunk) => chunks.push(chunk));
    child.on('error', reject);
    child.on('close', (code) => {
      if (code !== 0) return reject(new Error(`ffmpeg ${file}: exit ${code}`));
      const bytes = Buffer.concat(chunks);
      resolve(new Float32Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.length / 4)));
    });
  });
}

async function main() {
  const args = process.argv.slice(2);
  const option = (name) => {
    const at = args.indexOf(name);
    return at < 0 ? undefined : args.splice(at, 2)[1];
  };
  const [modelFile, out] = [option('--model'), option('--out')];
  if (!modelFile || !out || !args.length) throw new Error('usage: speaker-print.mjs --model <onnx> --out <voiceprint.json> <recording>...');
  const bytes = await readFile(modelFile);
  if (createHash('sha256').update(bytes).digest('hex') !== MODEL.sha256) throw new Error(`${modelFile}: not the pinned speaker model`);
  const session = await ort.InferenceSession.create(bytes, { intraOpNumThreads: 2, interOpNumThreads: 1 });
  const embed = async (features, frames) => Object.values(await session.run({ [session.inputNames[0]]: new ort.Tensor('float32', features, [1, frames, 80]) }))[0].data;
  const recordings = [];
  for (const [index, file] of args.entries()) {
    const audio = await decode(file);
    const enrollment = new SpeakerEnrollment({ embed, windows: Infinity });
    for (let at = 0; at < audio.length; at += CHUNK) await enrollment.push(audio.subarray(at, at + CHUNK));
    recordings.push(enrollment.embeddings);
    process.stderr.write(`${index + 1}/${args.length} recordings\r`);
  }
  const { print, windows, recordings: voiced } = dominantVoice(recordings);
  const temporary = `${out}.tmp`;
  await writeFile(temporary, JSON.stringify({ model: MODEL.sha256, print: Array.from(print), off: false }));
  await rename(temporary, out);
  console.log(`voiceprint from ${windows} windows in ${voiced} of ${args.length} recordings`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();
