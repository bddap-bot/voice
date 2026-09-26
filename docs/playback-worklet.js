import { PlaybackBuffer } from './playback-buffer.js';

class LivePlaybackProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buffer = new PlaybackBuffer(sampleRate * 180, Math.ceil(sampleRate * 0.15));
    this.failed = false;
    this.quiet = null;
    this.port.onmessage = ({ data }) => {
      if (data.type === 'clear') this.buffer.clear();
      if (data.type === 'quiet') {
        this.quiet = Math.round(sampleRate * data.ms / 1000);
        this.buffer.silentRun = 0;
      }
    };
  }
  process(inputs, outputs) {
    const output = outputs[0][0];
    if (this.failed) { output.fill(0); return true; }
    try { this.buffer.process(inputs[0]?.[0] ?? new Float32Array(output.length), output); }
    catch (error) { this.failed = true; output.fill(0); this.port.postMessage({ error: error.message }); }
    if (this.quiet !== null && this.buffer.quiet(this.quiet)) {
      this.quiet = null;
      this.port.postMessage({ quiet: true });
    }
    return true;
  }
}
registerProcessor('live-playback', LivePlaybackProcessor);
