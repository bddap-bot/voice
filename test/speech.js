import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { NAME } from '../docs/identity.js';

export const ordinarySpeech = [
  'I think we should get pizza tonight.',
  'Corvus is a genus of birds that includes crows and ravens.',
  'It is time to wake up, everyone.',
  'Can you hand me the remote?',
  'The weather should clear up by the weekend.',
  'Thanks, goodbye.',
];

export const farewell = `Thank you, ${NAME}. Goodbye.`;

export const voices = [
  { voice: 'en-us', speed: 150 },
  { voice: 'en-us', speed: 180 },
  { voice: 'en-gb', speed: 150 },
  { voice: 'en-us+f3', speed: 150 },
];

export async function synthesize(text, { voice, speed } = voices[0]) {
  try {
    const { stdout } = await promisify(execFile)('espeak-ng', ['-v', voice, '-s', String(speed), '--stdout', text], { encoding: 'buffer', maxBuffer: 16 * 1024 * 1024 });
    return stdout;
  } catch (error) {
    if (error.code === 'ENOENT') throw new Error("espeak-ng is required; run under nix-shell -p espeak-ng --run '…'");
    throw error;
  }
}

export function samples16k(wav) {
  let offset = 12;
  let rate = 0;
  let data = null;
  while (offset + 8 <= wav.length) {
    const id = wav.toString('ascii', offset, offset + 4);
    const size = wav.readUInt32LE(offset + 4);
    if (id === 'fmt ') rate = wav.readUInt32LE(offset + 12);
    if (id === 'data') data = wav.subarray(offset + 8, Math.min(wav.length, offset + 8 + size));
    offset += 8 + size + (size & 1);
  }
  const source = Float32Array.from({ length: data.length >> 1 }, (_, index) => data.readInt16LE(index * 2) / 32768);
  return Float32Array.from({ length: Math.floor(source.length * 16000 / rate) }, (_, index) => {
    const position = index * rate / 16000;
    const whole = Math.floor(position);
    const fraction = position - whole;
    return (source[whole] ?? 0) * (1 - fraction) + (source[whole + 1] ?? 0) * fraction;
  });
}
