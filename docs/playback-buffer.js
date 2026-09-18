export class PlaybackBuffer {
  constructor(capacity, delay = 0) {
    this.samples = new Float32Array(capacity);
    this.delay = delay;
    this.read = 0;
    this.size = 0;
    this.held = false;
  }
  clear() { this.read = 0; this.size = 0; }
  process(input, output) {
    output.fill(0);
    for (const sample of input) {
      if (this.size === this.samples.length) throw new Error('Live playback buffer overflow');
      this.samples[(this.read + this.size++) % this.samples.length] = sample;
    }
    while (this.size > this.delay + output.length) {
      let silent = true;
      for (let i = 0; i < output.length; i++) {
        if (Math.abs(this.samples[(this.read + i) % this.samples.length]) > 0.001) { silent = false; break; }
      }
      if (!silent) break;
      this.read = (this.read + output.length) % this.samples.length;
      this.size -= output.length;
    }
    if (this.held) return;
    for (let i = 0; i < output.length && this.size > this.delay; i++) {
      output[i] = this.samples[this.read];
      this.read = (this.read + 1) % this.samples.length;
      this.size--;
    }
  }
}
