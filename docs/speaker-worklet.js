import { CHUNK, GrantedAudio } from './speaker.js';

class SpeakerGateProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.audio = new GrantedAudio();
    this.chunk = new Float32Array(CHUNK);
    this.filled = 0;
    this.port.onmessage = ({ data }) => ('open' in data ? this.audio.open(data.open) : this.audio.close(data.close));
  }
  process([input], [output]) {
    const samples = input[0] ?? new Float32Array(output[0].length);
    this.audio.write(samples);
    for (const sample of samples) {
      this.chunk[this.filled++] = sample;
      if (this.filled < CHUNK) continue;
      this.port.postMessage(this.chunk, [this.chunk.buffer]);
      this.chunk = new Float32Array(CHUNK);
      this.filled = 0;
    }
    this.audio.read(output[0]);
    return true;
  }
}
registerProcessor('speaker-gate', SpeakerGateProcessor);
