import { CHUNK } from './wake.js';

class WakeCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.chunk = new Float32Array(CHUNK);
    this.filled = 0;
    this.port.onmessage = ({ data }) => { this.chunks = data; };
  }
  process([input]) {
    for (const sample of input[0] ?? []) {
      this.chunk[this.filled++] = sample;
      if (this.filled < CHUNK) continue;
      this.chunks?.postMessage(this.chunk, [this.chunk.buffer]);
      this.chunk = new Float32Array(CHUNK);
      this.filled = 0;
    }
    return true;
  }
}
registerProcessor('wake-capture', WakeCapture);
