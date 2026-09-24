import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ort from 'onnxruntime-node';
import { NAME, WAKE_PHRASE } from '../docs/identity.js';
import { CHUNK, RATE, WIDTH, WINDOW, WakeDecision, headScore, wakeFeatures } from '../docs/wake.js';

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const cache = process.env.VOICE_WAKE_CACHE ?? path.join(root, 'node_modules/.cache/wake');
const PIPER = {
  'en_US-libritts_r-medium': '10bb85e071d616fcf4071f369f1799d0491492ab3c5d552ec19fb548fac13195',
  'en_GB-vctk-medium': '4e9fc85ab9009385319fc6bae7f55577f8a2d7ee77fd9159a5500eb6531f41e6',
  'en_US-l2arctic-medium': 'd89f6f124bf1e7735b2179d2141b8001c3e19169d5e743ed6e35624f4c76f044',
  'en_US-arctic-medium': '483303e294947a3ec2f910ea96093d876e1640f5772e9d89e511d6c82c667286',
  'en_US-hfc_male-medium': 'd11e403a02bdf5a670c877b3dc56e0e1c8cece6fb30289586314dffdc0a78cb0',
  'en_US-ryan-high': 'b3990d7606e183ec8dbfba70a4607074f162de1a0c412e0180d1ff60bb154eca',
  'en_US-joe-medium': '58afce0321b8d9c46d7cdf9c16500cc55a793b4220212dba6b70fb788b3baf06',
  'en_US-john-medium': '789c6c875726e627ddee93d51d8727859abe9c091c3d141591f4b83c2072e988',
  'en_US-bryce-medium': 'dc9caa6c313199ffb5ac698b6e542fa6cba388aeaf2731e25262e33b9810aef1',
  'en_US-norman-medium': 'b9739443232a80a59c7d18810dd856899bf16a7964725f5ab81ea49b1351cb71',
  'en_US-lessac-medium': '5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f',
  'en_US-amy-medium': 'b3a6e47b57b8c7fbe6a0ce2518161a50f59a9cdd8a50835c02cb02bdd6206c18',
  'en_US-kristin-medium': '5849957f929cbf720c258f8458692d6103fff2f0e3d3b19c8259474bb06a18d4',
  'en_US-hfc_female-medium': '914c473788fc1fa8b63ace1cdcdb44588f4ae523d3ab37df1536616835a140b7',
  'en_GB-alan-medium': '0a309668932205e762801f1efc2736cd4b0120329622adf62be09e56339d3330',
  'en_GB-northern_english_male-medium': '57a219ae8e638873db7d18893304be5069c42868f392bb95c3ff17f0690d0689',
  'en_GB-cori-high': '470b4dd634c98f8a4850d7626ffc3dfc90774628eeef6605a6dd8f88f30a5903',
  'en_GB-alba-medium': '401369c4a81d09fdd86c32c5c864440811dbdcc66466cde2d64f7133a66ad03b',
};
const LIBRISPEECH = {
  'dev-clean': '76f87d090650617fca0cac8f88b9416e0ebf80350acb97b343a85fa903728ab3',
  'test-clean': '39fde525e59672dc6d1551919b1478f724438a95aa55f874b576be21967e6c23',
};
const FEATURES = 'https://huggingface.co/datasets/davidscripka/openwakeword_features/resolve/985bf1b47e7f19c07741af82bfe32d5a9dc56096';
const VALIDATION_FEATURES = 'a56a8a0f8e0efb91900acc6de4c0cdf4c564842e8475a7d49b36c039e17a690f';
const SPEAKERS = { 'en_US-libritts_r-medium': 904, 'en_GB-vctk-medium': 109, 'en_US-l2arctic-medium': 24, 'en_US-arctic-medium': 18 };
const VOICES = [...Object.keys(PIPER).filter((name) => !SPEAKERS[name]), 'espeak-en-us', 'flite-rms', 'flite-slt'];
export const HELD_OUT = new Set(['en_US-ryan-high', 'en_US-amy-medium', 'en_GB-alan-medium', 'en_GB-cori-high', 'flite-slt']);
const VALIDATION = new Set(['en_US-kristin-medium', 'en_GB-northern_english_male-medium']);
const SPEEDS = [0.85, 1, 1.2];
const TRAIN_SPEEDS = [0.8, 0.95, 1.1, 1.3];
const HARVARD = [
  'The birch canoe slid on the smooth planks.', 'Glue the sheet to the dark blue background.', "It's easy to tell the depth of a well.",
  'These days a chicken leg is a rare dish.', 'Rice is often served in round bowls.', 'The juice of lemons makes fine punch.',
  'The box was thrown beside the parked truck.', 'The hogs were fed chopped corn and garbage.', 'Four hours of steady work faced us.',
  'A large size in stockings is hard to sell.', 'The boy was there when the sun rose.', 'A rod is used to catch pink salmon.',
  'The source of the huge river is the clear spring.', 'Kick the ball straight and follow through.', 'Help the woman get back to her feet.',
  'A pot of tea helps to pass the evening.', 'Smoky fires lack flame and heat.', "The soft cushion broke the man's fall.",
  'The salt breeze came across from the sea.', 'The girl at the booth sold fifty bonds.', 'The small pup gnawed a hole in the sock.',
  'The fish twisted and turned on the bent hook.', 'Press the pants and sew a button on the vest.', 'The swan dive was far short of perfect.',
  'The beauty of the view stunned the young boy.', 'Two blue fish swam in the tank.', 'Her purse was full of useless trash.',
  'The colt reared and threw the tall rider.', 'It snowed, rained, and hailed the same morning.', 'Read verse out loud for pleasure.',
];
const TRAIN_SENTENCES = HARVARD.slice(0, 20);
export const HELD_OUT_SENTENCES = HARVARD.slice(20);
const NEAR_MISS = [
  'Hark, the herald angels sing.', 'Quote the raven, nevermore.',
  'Something wicked this way comes.', 'Come out, come out, wherever you are.', 'The cows are hungry this morning.',
  'Careful, the candle is lit.', 'Klaatu barada necktie.', 'The hour has come to leave.', 'I summon the dragon card.',
];
const SOUNDALIKES = ['Marcus', 'Cyrus', 'Corbin', 'Carlos', 'Horace', 'Chorus'];
const PRE = Math.round(2.2 * RATE);
const POST = Math.round(0.8 * RATE);
const SESSION_GAP_MS = 20 * 60 * 1000;
const HIDDEN = 32;
const BLOCK_SECONDS = 600;
const TARGET_FALSE_ACCEPTS = 0.25;

function hash(value) {
  return createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 32);
}

function random(seed) {
  let state = parseInt(hash(seed).slice(0, 8), 16) || 1;
  return () => {
    state ^= state << 13; state ^= state >>> 17; state ^= state << 5;
    return (state >>> 0) / 4294967296;
  };
}

function run(command, args, input = '') {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args);
    const output = [];
    let errors = '';
    child.stdout.on('data', (data) => output.push(data));
    child.stderr.on('data', (data) => { errors += data; });
    child.on('error', (error) => reject(error.code === 'ENOENT' ? new Error(`${command} is required on PATH`) : error));
    child.on('close', (code) => code === 0 ? resolve({ stdout: Buffer.concat(output), stderr: errors }) : reject(new Error(`${command} ${args.join(' ')} exited ${code}: ${errors.slice(-2000)}`)));
    child.stdin.end(input);
  });
}

async function pool(items, limit, work) {
  const results = new Array(items.length);
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const index = next++;
      results[index] = await work(items[index], index);
    }
  }));
  return results;
}

const downloads = new Map();

function download(url, sha256, file) {
  if (!downloads.has(file)) downloads.set(file, fetchOnce(url, sha256, file));
  return downloads.get(file);
}

async function fetchOnce(url, sha256, file) {
  if (existsSync(file)) return file;
  await mkdir(path.dirname(file), { recursive: true });
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const actual = createHash('sha256').update(bytes).digest('hex');
  if (sha256 && actual !== sha256) throw new Error(`${url} has sha256 ${actual}, expected ${sha256}`);
  await writeFile(`${file}.part`, bytes);
  await rename(`${file}.part`, file);
  return file;
}

async function piperVoice(name) {
  const [locale, voice, quality] = name.split('-');
  const base = `https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/${locale.split('_')[0]}/${locale}/${voice}/${quality}/${name}`;
  const model = await download(`${base}.onnx`, PIPER[name], path.join(cache, 'voices', `${name}.onnx`));
  await download(`${base}.onnx.json`, null, `${model}.json`);
  return model;
}

async function decode(input, stdin = '') {
  const { stdout } = await run('ffmpeg', ['-hide_banner', '-loglevel', 'error', ...input, '-ac', '1', '-ar', String(RATE), '-f', 'f32le', 'pipe:1'], stdin);
  return new Float32Array(stdout.buffer.slice(stdout.byteOffset, stdout.byteOffset + stdout.byteLength));
}

async function cached(key, produce, directory = cache) {
  const file = path.join(directory, 'audio', `${hash(key)}.f32`);
  if (existsSync(file)) {
    const bytes = await readFile(file);
    return new Float32Array(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  }
  const audio = await produce();
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, Buffer.from(audio.buffer, audio.byteOffset, audio.byteLength));
  return audio;
}

async function rendered(command, args, name) {
  await mkdir(cache, { recursive: true });
  const directory = await mkdtemp(path.join(cache, 'tts-'));
  try {
    await run(command, args(path.join(directory, name)));
    return await decode(['-i', path.join(directory, name)]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export async function synthesize(voice, speed, texts, speaker = null) {
  const model = voice.startsWith('en_') ? await piperVoice(voice) : null;
  const config = model && JSON.parse(await readFile(`${model}.json`, 'utf8'));
  const clips = [];
  for (const text of texts) clips.push(await cached(['tts', voice, speaker, speed, text], () => {
    if (model) return rendered('piper', (out) => ['-m', model, '-f', out, '--length-scale', String(config.inference.length_scale / speed), ...(speaker === null ? [] : ['-s', String(speaker)]), '--', text], 'clip.wav');
    if (voice === 'espeak-en-us') return rendered('espeak-ng', (out) => ['-v', 'en-us', '-s', String(Math.round(175 * speed)), '-w', out, text], 'clip.wav');
    if (voice.startsWith('flite-')) return rendered('flite', (out) => ['-voice', voice.slice(6), '--setf', `duration_stretch=${(1 / speed).toFixed(4)}`, '-t', text, '-o', out], 'clip.wav');
    throw new Error(`no synthesizer for ${voice}`);
  }));
  return clips;
}

const HALF = Float32Array.from({ length: 65536 }, (_, bits) => {
  const sign = bits & 0x8000 ? -1 : 1;
  const exponent = (bits >> 10) & 0x1f;
  const fraction = bits & 0x3ff;
  if (exponent === 31) return fraction ? NaN : sign * Infinity;
  return exponent ? sign * 2 ** (exponent - 15) * (1 + fraction / 1024) : sign * 2 ** -14 * (fraction / 1024);
});

async function npy(file) {
  const bytes = await readFile(file);
  const version = bytes[6];
  const length = version === 1 ? bytes.readUInt16LE(8) : bytes.readUInt32LE(8);
  const start = (version === 1 ? 10 : 12) + length;
  const header = bytes.toString('latin1', version === 1 ? 10 : 12, start);
  const type = /'descr': '([^']+)'/.exec(header)[1];
  const shape = /'shape': \(([^)]*)\)/.exec(header)[1].split(',').filter((part) => part.trim()).map(Number);
  const data = bytes.buffer.slice(bytes.byteOffset + start, bytes.byteOffset + bytes.length);
  const values = type === '<f4' ? new Float32Array(data, 0, Math.floor(data.byteLength / 4)) : new Uint16Array(data, 0, Math.floor(data.byteLength / 2));
  const width = shape.slice(1).reduce((product, size) => product * size, 1);
  return values.subarray(0, Math.floor(values.length / width) * width);
}

async function acav(megabytes) {
  const file = path.join(cache, `acav-${megabytes}mb.npy`);
  if (!existsSync(file)) {
    const response = await fetch(`${FEATURES}/openwakeword_features_ACAV100M_2000_hrs_16bit.npy`, { headers: { range: `bytes=0-${megabytes * 1024 * 1024 - 1}` } });
    if (response.status !== 206) throw new Error(`ACAV100M features answered ${response.status}`);
    await mkdir(cache, { recursive: true });
    await writeFile(`${file}.part`, Buffer.from(await response.arrayBuffer()));
    await rename(`${file}.part`, file);
  }
  return npy(file);
}

async function librispeech(set) {
  const archive = await download(`https://www.openslr.org/resources/12/${set}.tar.gz`, LIBRISPEECH[set], path.join(cache, 'librispeech', `${set}.tar.gz`));
  const directory = path.join(cache, 'librispeech', set);
  if (!existsSync(path.join(directory, 'LibriSpeech'))) {
    await mkdir(directory, { recursive: true });
    await run('tar', ['-xzf', archive, '-C', directory]);
  }
  const speakers = path.join(directory, 'LibriSpeech', set);
  const chapters = [];
  for (const speaker of (await readdir(speakers)).sort()) {
    for (const chapter of (await readdir(path.join(speakers, speaker))).sort()) {
      const folder = path.join(speakers, speaker, chapter);
      chapters.push({ key: ['librispeech', set, speaker, chapter], label: `${set} ${speaker}-${chapter}`, audio: async () => {
        const files = (await readdir(folder)).filter((name) => name.endsWith('.flac')).sort();
        const list = path.join(folder, 'concat.txt');
        await writeFile(list, files.map((name) => `file '${path.join(folder, name)}'`).join('\n'));
        return decode(['-f', 'concat', '-safe', '0', '-i', list]);
      } });
    }
  }
  return chapters;
}

function level(audio) {
  let sum = 0;
  for (const sample of audio) sum += sample * sample;
  return Math.sqrt(sum / Math.max(1, audio.length));
}

export function speechSpans(audio, gap) {
  const frame = RATE / 50;
  const energies = [];
  for (let start = 0; start + frame <= audio.length; start += frame) energies.push(level(audio.subarray(start, start + frame)));
  const sorted = [...energies].sort((a, b) => a - b);
  const threshold = Math.max(sorted[Math.floor(sorted.length * 0.1)] * 4, sorted[sorted.length - 1] * 0.06, 1e-4);
  const spans = [];
  energies.forEach((energy, index) => {
    if (energy < threshold) return;
    const last = spans.at(-1);
    if (last && index * frame - last.end <= gap * RATE) last.end = (index + 1) * frame;
    else spans.push({ start: index * frame, end: (index + 1) * frame });
  });
  return spans.filter((span) => span.end - span.start >= 0.25 * RATE);
}

function gaussian(rng) {
  return Math.sqrt(-2 * Math.log(rng() || 1e-12)) * Math.cos(2 * Math.PI * rng());
}

function noise(length, rms, rng, color = 'white') {
  const out = new Float32Array(length);
  let brown = 0;
  let pink = [0, 0, 0];
  for (let index = 0; index < length; index++) {
    const white = gaussian(rng);
    if (color === 'white') out[index] = white;
    else if (color === 'brown') out[index] = brown = 0.98 * brown + 0.2 * white;
    else {
      pink = [0.99765 * pink[0] + white * 0.099046, 0.963 * pink[1] + white * 0.2965164, 0.57 * pink[2] + white * 1.0526913];
      out[index] = pink[0] + pink[1] + pink[2] + white * 0.1848;
    }
  }
  const scale = rms / (level(out) || 1);
  return out.map((sample) => sample * scale);
}

function fft(re, im, invert) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { [re[i], re[j]] = [re[j], re[i]]; [im[i], im[j]] = [im[j], im[i]]; }
  }
  for (let length = 2; length <= n; length <<= 1) {
    const angle = (invert ? 2 : -2) * Math.PI / length;
    const wr = Math.cos(angle);
    const wi = Math.sin(angle);
    for (let start = 0; start < n; start += length) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < length / 2; k++) {
        const ar = re[start + k + length / 2] * cr - im[start + k + length / 2] * ci;
        const ai = re[start + k + length / 2] * ci + im[start + k + length / 2] * cr;
        re[start + k + length / 2] = re[start + k] - ar;
        im[start + k + length / 2] = im[start + k] - ai;
        re[start + k] += ar;
        im[start + k] += ai;
        [cr, ci] = [cr * wr - ci * wi, cr * wi + ci * wr];
      }
    }
  }
  if (invert) for (let index = 0; index < n; index++) { re[index] /= n; im[index] /= n; }
}

function reverberate(audio, rng) {
  const tail = 0.2 + rng() * 0.5;
  const room = new Float64Array(Math.round(tail * RATE));
  const direct = 0.3 + rng() * 0.7;
  for (let index = 1; index < room.length; index++) room[index] = gaussian(rng) * Math.exp(-6.9 * index / room.length) * 0.08;
  room[0] = direct;
  let size = 1;
  while (size < audio.length + room.length) size <<= 1;
  const [ar, ai, br, bi] = [new Float64Array(size), new Float64Array(size), new Float64Array(size), new Float64Array(size)];
  ar.set(audio);
  br.set(room);
  fft(ar, ai, false);
  fft(br, bi, false);
  for (let index = 0; index < size; index++) [ar[index], ai[index]] = [ar[index] * br[index] - ai[index] * bi[index], ar[index] * bi[index] + ai[index] * br[index]];
  fft(ar, ai, true);
  const out = Float32Array.from(ar.subarray(0, audio.length));
  const scale = level(audio) / (level(out) || 1);
  return out.map((sample) => sample * scale);
}

function placed(clip, rng, floor = 3e-4) {
  const out = noise(PRE + clip.length + POST, floor, rng);
  for (let index = 0; index < clip.length; index++) out[PRE + index] += clip[index];
  return out;
}

function augmented(audio, rng, babble) {
  let out = rng() < 0.5 ? reverberate(audio, rng) : Float32Array.from(audio);
  const speech = level(out);
  if (rng() < 0.8) {
    const snr = 5 + rng() * 20;
    const rms = speech / 10 ** (snr / 20);
    let background;
    if (rng() < 0.4) {
      const source = babble.audio[Math.floor(rng() * babble.audio.length)];
      const start = Math.floor(rng() * Math.max(1, source.length - out.length));
      background = source.subarray(start, start + out.length);
      const scale = rms / (level(background) || 1);
      background = background.map((sample) => sample * scale);
    } else background = noise(out.length, rms, rng, ['white', 'pink', 'brown'][Math.floor(rng() * 3)]);
    out = out.map((sample, index) => sample + (background[index] ?? 0));
  }
  const gain = 10 ** ((-12 + rng() * 15) / 20);
  return out.map((sample) => Math.max(-1, Math.min(1, sample * gain)));
}

let features;

async function streamed(audio) {
  features ??= wakeFeatures(ort, (name) => ort.InferenceSession.create(path.join(root, 'docs/wake', `${name}.onnx`)));
  return (await features).all(audio);
}

async function embedded(stream) {
  if (stream.embeddings) return stream;
  const file = path.join(stream.cache ?? cache, 'features', `${hash(['from the first embedding', ...stream.key])}.f32`);
  if (existsSync(file)) {
    const bytes = await readFile(file);
    const data = new Float32Array(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
    return { ...stream, first: data[0], embeddings: data.subarray(1) };
  }
  const { first, embeddings } = await streamed(await stream.audio());
  const data = new Float32Array(embeddings.length + 1);
  data[0] = first;
  data.set(embeddings, 1);
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, Buffer.from(data.buffer));
  return { ...stream, first, embeddings };
}

function windowAt(embeddings, index, out = new Float32Array(WINDOW * WIDTH)) {
  out.set(embeddings.subarray((index - WINDOW + 1) * WIDTH, (index + 1) * WIDTH));
  return out;
}

function windowCount(stream) {
  return Math.max(0, stream.embeddings.length / WIDTH - WINDOW + 1);
}

function seconds(stream, index) {
  return (stream.first + index + 1) * CHUNK / RATE;
}

function positiveWindows(stream) {
  const out = [];
  for (let index = WINDOW - 1; index < stream.embeddings.length / WIDTH; index++) {
    const at = seconds(stream, index);
    if (stream.spans.some((span) => at >= span.end / RATE - 0.16 && at <= span.end / RATE + 0.4)) out.push(windowAt(stream.embeddings, index));
  }
  return out;
}

function scores(head, stream) {
  const out = new Float32Array(windowCount(stream));
  const window = new Float32Array(WINDOW * WIDTH);
  for (let index = 0; index < out.length; index++) out[index] = headScore(head, windowAt(stream.embeddings, index + WINDOW - 1, window));
  return out;
}

function wakes(stream, streamScores, threshold) {
  const decision = new WakeDecision({ threshold });
  const out = [];
  streamScores.forEach((score, index) => { if (decision.decide(score)?.wake) out.push(seconds(stream, index + WINDOW - 1)); });
  return out;
}

function recall(streams, threshold) {
  let detected = 0;
  let total = 0;
  const missed = [];
  for (const stream of streams) {
    const times = wakes(stream, stream.scores, threshold);
    for (const span of stream.spans) {
      total++;
      if (times.some((at) => at >= span.start / RATE && at <= span.end / RATE + 1)) detected++;
      else missed.push(stream.label);
    }
  }
  return { detected, total, rate: total ? detected / total : null, missed };
}

function falseAccepts(streams, threshold) {
  const accepted = streams.map((stream) => [stream.label, wakes(stream, stream.scores, threshold).length]).filter(([, count]) => count);
  const events = accepted.reduce((sum, [, count]) => sum + count, 0);
  const hours = streams.reduce((sum, stream) => sum + windowCount(stream), 0) * CHUNK / RATE / 3600;
  return { events, hours: Number(hours.toFixed(3)), perHour: hours ? Number((events / hours).toFixed(3)) : null, accepted: accepted.map(([label]) => label) };
}

function encode(values) {
  return Buffer.from(Float32Array.from(values).buffer).toString('base64');
}

function fit(positives, negatives, rng, windows, steps = 4000) {
  const inputs = WINDOW * WIDTH;
  const index = [];
  negatives.forEach((stream, which) => { for (let at = WINDOW - 1; at < stream.embeddings.length / WIDTH; at++) index.push(which, at); });
  const pairs = Int32Array.from(index);
  const fromStreams = pairs.length / 2;
  const count = fromStreams + windows.length / inputs;
  const negativeAt = (pick, out) => {
    if (pick < fromStreams) return windowAt(negatives[pairs[pick * 2]].embeddings, pairs[pick * 2 + 1], out);
    const base = (pick - fromStreams) * inputs;
    for (let i = 0; i < inputs; i++) out[i] = HALF[windows[base + i]];
    return out;
  };
  const mean = new Float64Array(inputs);
  const square = new Float64Array(inputs);
  const probe = new Float32Array(inputs);
  const samples = positives.length + Math.min(20000, count);
  const accumulate = (window) => { for (let i = 0; i < inputs; i++) { mean[i] += window[i]; square[i] += window[i] * window[i]; } };
  positives.forEach(accumulate);
  for (let drawn = positives.length; drawn < samples; drawn++) accumulate(negativeAt(Math.floor(rng() * count), probe));
  const head = {
    mean: Float32Array.from(mean, (sum) => sum / samples),
    scale: Float32Array.from(square, (sum, i) => 1 / (Math.sqrt(Math.max(1e-6, sum / samples - (mean[i] / samples) ** 2)) + 1e-3)),
    w1: Float32Array.from({ length: HIDDEN * inputs }, () => gaussian(rng) * Math.sqrt(2 / inputs)),
    b1: new Float32Array(HIDDEN),
    w2: Float32Array.from({ length: HIDDEN }, () => gaussian(rng) * Math.sqrt(1 / HIDDEN)),
    b2: 0,
  };
  const moments = ['w1', 'b1', 'w2'].map((name) => [name, new Float32Array(head[name].length), new Float32Array(head[name].length)]);
  let b2m = 0;
  let b2v = 0;
  const gradients = { w1: new Float32Array(head.w1.length), b1: new Float32Array(HIDDEN), w2: new Float32Array(HIDDEN), b2: 0 };
  const x = new Float32Array(inputs);
  const hidden = new Float32Array(HIDDEN);
  let hard = [];
  for (let step = 1; step <= steps; step++) {
    if (step % 1000 === 0 && step < steps) {
      const scored = [];
      const window = new Float32Array(inputs);
      for (let pick = 0; pick < count; pick++) scored.push([headScore(head, negativeAt(pick, window)), pick]);
      hard = scored.sort((a, b) => b[0] - a[0]).slice(0, 4096).map(([, pick]) => pick);
    }
    gradients.w1.fill(0);
    gradients.b1.fill(0);
    gradients.w2.fill(0);
    gradients.b2 = 0;
    const negativeWeight = 1 + 7 * step / steps;
    for (let item = 0; item < 256; item++) {
      const positive = item < 64;
      if (positive) x.set(positives[Math.floor(rng() * positives.length)]);
      else negativeAt(hard.length && rng() < 0.5 ? hard[Math.floor(rng() * hard.length)] : Math.floor(rng() * count), x);
      for (let i = 0; i < inputs; i++) x[i] = (x[i] - head.mean[i]) * head.scale[i];
      let logit = head.b2;
      for (let unit = 0; unit < HIDDEN; unit++) {
        let sum = head.b1[unit];
        const row = unit * inputs;
        for (let i = 0; i < inputs; i++) sum += head.w1[row + i] * x[i];
        hidden[unit] = sum > 0 ? sum : 0;
        logit += head.w2[unit] * hidden[unit];
      }
      const weight = positive ? 1 : negativeWeight;
      const delta = weight * (1 / (1 + Math.exp(-logit)) - (positive ? 1 : 0)) / 256;
      gradients.b2 += delta;
      for (let unit = 0; unit < HIDDEN; unit++) {
        gradients.w2[unit] += delta * hidden[unit];
        if (hidden[unit] <= 0) continue;
        const back = delta * head.w2[unit];
        gradients.b1[unit] += back;
        const row = unit * inputs;
        for (let i = 0; i < inputs; i++) gradients.w1[row + i] += back * x[i];
      }
    }
    const rate = 1e-3 * (step > steps * 0.75 ? 0.3 : 1);
    const correction1 = 1 - 0.9 ** step;
    const correction2 = 1 - 0.999 ** step;
    for (const [name, first, second] of moments) {
      const values = head[name];
      const gradient = gradients[name];
      for (let i = 0; i < values.length; i++) {
        const g = gradient[i] + (name === 'w1' ? 1e-4 * values[i] : 0);
        first[i] = 0.9 * first[i] + 0.1 * g;
        second[i] = 0.999 * second[i] + 0.001 * g * g;
        values[i] -= rate * (first[i] / correction1) / (Math.sqrt(second[i] / correction2) + 1e-8);
      }
    }
    b2m = 0.9 * b2m + 0.1 * gradients.b2;
    b2v = 0.999 * b2v + 0.001 * gradients.b2 * gradients.b2;
    head.b2 -= rate * (b2m / correction1) / (Math.sqrt(b2v / correction2) + 1e-8);
  }
  return head;
}

function variants(phrase) {
  const words = phrase.split(/\s+/).filter(Boolean);
  const out = new Set([NAME]);
  if (words.length > 1) words.forEach((_, drop) => out.add(words.filter((__, index) => index !== drop).join(' ')));
  if (phrase.includes(NAME)) for (const name of SOUNDALIKES) out.add(phrase.replaceAll(NAME, name));
  out.delete(phrase);
  return [...out];
}

function clipStream(key, clip, spans, rng, babble = null, speed = null) {
  return { key: babble ? [...key, babble.label] : key, speed, label: key.filter((part) => part !== null && part !== 'synthetic').join(' '), spans: spans.map((span) => ({ start: span.start + PRE, end: span.end + PRE })), audio: async () => placed(babble ? augmented(clip, rng, babble) : clip, rng) };
}

async function spoken(voice, speed, texts, speaker = null) {
  return (await synthesize(voice, speed, texts, speaker)).map((clip, index) => ({ voice, speed, speaker, text: texts[index], clip }));
}

async function synthetic(phrase) {
  const negatives = (sentences) => [...NEAR_MISS, ...variants(phrase), ...sentences];
  const trainVoices = VOICES.filter((voice) => !HELD_OUT.has(voice) && !VALIDATION.has(voice));
  const heldVoices = VOICES.filter((voice) => HELD_OUT.has(voice));
  const speakers = (voice, keep) => Array.from({ length: SPEAKERS[voice] }, (_, speaker) => speaker).filter(keep);
  const jobs = [
    ...trainVoices.flatMap((voice) => TRAIN_SPEEDS.map((speed) => ['train', voice, speed, [phrase], null])),
    ...trainVoices.map((voice) => ['train', voice, 1, negatives(TRAIN_SENTENCES), null]),
    ...heldVoices.flatMap((voice) => SPEEDS.map((speed) => ['eval', voice, speed, [phrase, ...negatives(HELD_OUT_SENTENCES)], null])),
    ...[...VALIDATION].flatMap((voice) => SPEEDS.map((speed) => ['validation', voice, speed, speed === 1 ? [phrase, ...negatives(TRAIN_SENTENCES)] : [phrase], null])),
    ...Object.keys(SPEAKERS).flatMap((voice) => [
      ...speakers(voice, (speaker) => speaker % 10 >= 2 && (SPEAKERS[voice] < 200 || speaker % 6 === 0)).map((speaker, index) => ['train', voice, TRAIN_SPEEDS[index % TRAIN_SPEEDS.length], index % 6 ? [phrase] : [phrase, ...negatives(TRAIN_SENTENCES)], speaker]),
      ...speakers(voice, (speaker) => speaker % 10 === 1).map((speaker) => ['validation', voice, 1, [phrase], speaker]),
      ...speakers(voice, (speaker) => speaker % 10 === 0).map((speaker, index) => ['eval', voice, SPEEDS[index % 3], [phrase], speaker]),
    ]),
  ];
  let done = 0;
  const results = await pool(jobs, Math.max(2, Math.floor(os.availableParallelism() / 3)), async ([split, voice, speed, texts, speaker]) => {
    const clips = (await spoken(voice, speed, texts, speaker)).map((item) => ({ ...item, split }));
    if (++done % 50 === 0 || done === jobs.length) console.log(`synthesized ${done} of ${jobs.length} voice and speed jobs`);
    return clips;
  });
  return results.flat();
}

async function real(corpus, phrase) {
  const manifest = JSON.parse(await readFile(path.join(corpus, 'manifest.json'), 'utf8'));
  const label = (text) => text?.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
  const clips = manifest.clips.filter((item) => item.phrase === null || label(item.phrase) === label(phrase));
  return Promise.all(clips.map(async (item) => {
    const file = path.join(corpus, item.file);
    const key = ['real', item.file, (await stat(file)).mtimeMs];
    return { ...item, key, clip: await cached(key, () => decode(['-i', file]), path.join(corpus, 'cache')) };
  }));
}

function inside(directory, candidate) {
  const relative = path.relative(directory, path.resolve(candidate));
  return !relative.startsWith('..') && !path.isAbsolute(relative);
}

async function train({ phrase, corpus, acavMegabytes, out }) {
  if ((corpus || acavMegabytes) && inside(root, out)) throw new Error('a model trained on recordings or on the non-commercial ACAV100M features stays private; write it outside the repository');
  const rng = random(['train', phrase]);
  const [spokenClips, recorded, devClean, testClean] = await Promise.all([synthetic(phrase), corpus ? real(corpus, phrase) : [], librispeech('dev-clean'), librispeech('test-clean')]);
  const babble = { label: devClean[1].label, audio: [await devClean[1].audio()] };
  const heldBabble = { label: testClean[0].label, audio: [await testClean[0].audio()] };
  const streams = { train: [], trainNegative: [], validation: [], validationNegative: [], eval: {}, evalNegative: {} };
  const add = (group, name, stream) => { (streams[group][name] ??= []).push(stream); };
  for (const item of spokenClips) {
    const positive = item.text === phrase;
    const spans = positive ? speechSpans(item.clip, 10).slice(-1) : [];
    const key = ['synthetic', item.voice, item.speaker, item.speed, item.text];
    const heldOutVoice = item.speaker === null ? 'held-out voices' : 'held-out speakers';
    if (item.split === 'train' && positive) {
      streams.train.push(clipStream([...key, 'clean'], item.clip, spans, random(key)));
      for (let copy = 1; copy <= (item.speaker === null ? 5 : 1); copy++) streams.train.push(clipStream([...key, 'augmented', copy], item.clip, spans, random([...key, 'augmented', copy]), babble));
    } else if (item.split === 'train') streams.trainNegative.push(clipStream([...key, 'clean'], item.clip, [], random(key)));
    else if (item.split === 'validation' && positive) {
      streams.validation.push(clipStream([...key, 'clean'], item.clip, spans, random(key)));
      streams.validation.push(clipStream([...key, 'validation noise'], item.clip, spans, random([...key, 'validation noise']), babble));
    } else if (item.split === 'validation') streams.validationNegative.push(clipStream([...key, 'clean'], item.clip, [], random(key)));
    else if (positive) {
      add('eval', heldOutVoice, clipStream([...key, 'clean'], item.clip, spans, random(key), null, item.speed));
      add('eval', `${heldOutVoice} in noise and rooms`, clipStream([...key, 'held-out noise'], item.clip, spans, random([...key, 'held-out noise']), heldBabble, item.speed));
    }
    else add('evalNegative', variants(phrase).includes(item.text) ? 'variants of the phrase, held-out voices' : 'Harvard and near-miss sentences, held-out voices', clipStream([...key, 'clean'], item.clip, [], random(key)));
  }
  for (const [index, rms] of [0, 1e-5, 3e-4, 3e-3, 3e-2, 1e-1].entries()) {
    for (const color of ['white', 'pink', 'brown']) {
      const key = ['noise', color, rms];
      streams.trainNegative.push({ key, label: key.join(' '), audio: async () => noise(30 * RATE, rms, random([...key, index]), color) });
    }
  }
  devClean.forEach((chapter, index) => (index % 5 ? streams.trainNegative : streams.validationNegative).push(chapter));
  streams.evalNegative['LibriSpeech test-clean'] = testClean;
  const realAudio = await npy(await download(`${FEATURES}/validation_set_features.npy`, VALIDATION_FEATURES, path.join(cache, 'validation_set_features.npy')));
  const block = BLOCK_SECONDS * RATE / CHUNK;
  for (let start = 0, index = 0; start * WIDTH < realAudio.length; start += block, index++) {
    const stream = { key: ['validation set', start], label: `validation set from ${(start * CHUNK / RATE / 60).toFixed(0)} min`, first: 0, embeddings: realAudio.subarray(start * WIDTH, Math.min(realAudio.length, (start + block) * WIDTH)) };
    if (index % 2) add('evalNegative', 'openWakeWord false-positive validation set, odd ten-minute blocks: dinner parties, conversation, reverberated music', stream);
    else streams.validationNegative.push(stream);
  }
  for (const item of recorded) {
    const spans = item.phrase === null ? [] : speechSpans(item.clip, 0.9);
    const { key } = item;
    const own = (stream) => ({ ...stream, cache: path.join(corpus, 'cache') });
    if (item.split === 'train' && item.phrase !== null) {
      streams.train.push(own(clipStream([...key, 'clean'], item.clip, spans, random(key))));
      for (const copy of [1, 2]) streams.train.push(own(clipStream([...key, 'augmented', copy], item.clip, spans, random([...key, copy]), babble)));
    } else if (item.split === 'train') streams.trainNegative.push(own({ key, label: item.file, audio: async () => item.clip }));
    else if (item.phrase !== null) add('eval', 'recorded session', own(clipStream([...key, 'clean'], item.clip, spans, random(key))));
    else add('evalNegative', 'recorded ordinary speech', own({ key, label: item.file, audio: async () => item.clip }));
  }
  const extract = async (list) => {
    const out = await pool(list, 3, (stream) => embedded({ ...stream, key: ['features', ...stream.key] }));
    console.log(`features for ${out.length} streams`);
    return out;
  };
  const positives = (await extract(streams.train)).flatMap(positiveWindows);
  const negatives = await extract(streams.trainNegative);
  console.log(`training on ${positives.length} positive windows and ${negatives.reduce((sum, stream) => sum + windowCount(stream), 0)} negative windows`);
  const head = fit(positives, negatives, rng, acavMegabytes ? await acav(acavMegabytes) : new Uint16Array(0));
  const scored = async (list) => (await extract(list)).map((stream) => ({ ...stream, scores: scores(head, stream) }));
  const validation = await scored(streams.validation);
  const validationNegative = await scored(streams.validationNegative);
  const candidates = [0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.97, 0.99, 0.995, 0.998, 0.999].map((threshold) => {
    const { events, hours, perHour } = falseAccepts(validationNegative, threshold);
    return { threshold, recall: recall(validation, threshold).rate, events, hours, perHour };
  });
  const threshold = (candidates.find((row) => row.perHour <= TARGET_FALSE_ACCEPTS) ?? candidates.at(-1)).threshold;
  const metrics = { validation: candidates, recall: {}, falseAccepts: {} };
  for (const [name, list] of Object.entries(streams.eval)) {
    const evaluated = await scored(list);
    metrics.recall[name] = recall(evaluated, threshold);
    if (name !== 'recorded session') metrics.recall[name].bySpeed = Object.fromEntries(SPEEDS.map((speed) => [speed, recall(evaluated.filter((stream) => stream.speed === speed), threshold)]));
  }
  for (const [name, list] of Object.entries(streams.evalNegative)) metrics.falseAccepts[name] = falseAccepts(await scored(list), threshold);
  const model = { phrase, threshold, mean: encode(head.mean), scale: encode(head.scale), w1: encode(head.w1), b1: encode(head.b1), w2: encode(head.w2), b2: head.b2, metrics };
  await mkdir(path.dirname(out), { recursive: true });
  await writeFile(out, `${JSON.stringify(model, (key, value) => key === 'missed' || key === 'accepted' ? undefined : value)}\n`);
  console.log(JSON.stringify({ threshold, ...metrics }, null, 2));
  return model;
}

export async function ingest(directory, { corpus, phrase, ordinary }) {
  if (!directory || !corpus || Boolean(phrase) === ordinary) throw new Error('usage: node scripts/wake.mjs ingest <clips> --corpus <dir> (--phrase <text> | --ordinary)');
  if (inside(root, corpus)) throw new Error('recordings are private; keep the corpus outside the repository');
  const files = await Promise.all((await readdir(directory)).map(async (name) => ({ name, time: (await stat(path.join(directory, name))).mtimeMs })));
  const sessions = [];
  for (const file of files.sort((a, b) => a.time - b.time)) {
    const last = sessions.at(-1);
    if (last && file.time - last.at(-1).time < SESSION_GAP_MS) last.push(file);
    else sessions.push([file]);
  }
  if (sessions.length < 2) throw new Error(`found ${sessions.length} recording session; at least two are needed to hold one out`);
  const manifestFile = path.join(corpus, 'manifest.json');
  const manifest = existsSync(manifestFile) ? JSON.parse(await readFile(manifestFile, 'utf8')) : { clips: [] };
  const kind = ordinary ? 'ordinary' : hash(phrase).slice(0, 12);
  const held = sessions.length - Math.max(1, Math.round(sessions.length / 4));
  for (const [index, session] of sessions.entries()) {
    const id = new Date(session[0].time).toISOString().replace(/[:.]/g, '-');
    await mkdir(path.join(corpus, kind, id), { recursive: true });
    for (const file of session) {
      const relative = path.join(kind, id, file.name);
      await copyFile(path.join(directory, file.name), path.join(corpus, relative));
      manifest.clips = manifest.clips.filter((item) => item.file !== relative);
      manifest.clips.push({ file: relative, phrase: ordinary ? null : phrase, session: id, split: index >= held ? 'eval' : 'train' });
    }
  }
  await writeFile(manifestFile, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(sessions.map((session, index) => `${index >= held ? 'eval' : 'train'} session of ${session.length} clips`).join('\n'));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, ...args] = process.argv.slice(2);
  const option = (name) => { const at = args.indexOf(name); return at >= 0 ? args[at + 1] : undefined; };
  if (command === 'train') await train({ phrase: option('--phrase') ?? WAKE_PHRASE, corpus: option('--corpus'), acavMegabytes: Number(option('--acav') ?? 0), out: path.resolve(option('--out') ?? path.join(root, 'scripts/wake-demo.json')) });
  else if (command === 'ingest') await ingest(args[0], { corpus: option('--corpus'), phrase: option('--phrase'), ordinary: args.includes('--ordinary') });
  else throw new Error('usage: node scripts/wake.mjs train [--phrase <text>] [--corpus <dir>] [--acav <megabytes>] [--out <file>] | ingest <clips> --corpus <dir> (--phrase <text> | --ordinary)');
}
