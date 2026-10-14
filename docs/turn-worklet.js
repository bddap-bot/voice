const CHUNK = 1600;

class TurnTapProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.chunk = new Float32Array(CHUNK);
    this.filled = 0;
    this.port.onmessage = ({ data }) => {
      if (data.audio) this.audio = data.audio;
      if (data.close) this.audio = this.audio?.close();
    };
  }
  process([input]) {
    const samples = input[0];
    if (!samples || !this.audio) return true;
    for (const sample of samples) {
      this.chunk[this.filled++] = sample;
      if (this.filled < CHUNK) continue;
      this.audio.postMessage(this.chunk, [this.chunk.buffer]);
      this.chunk = new Float32Array(CHUNK);
      this.filled = 0;
    }
    return true;
  }
}
registerProcessor('turn-tap', TurnTapProcessor);
