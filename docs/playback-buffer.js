const SILENT = 0.001;

export class PlaybackBuffer {
  constructor(capacity, delay = 0) {
    this.samples = new Float32Array(capacity);
    this.delay = delay;
    this.read = 0;
    this.size = 0;
    this.silentRun = 0;
    this.silenced = false;
  }
  clear() { this.read = 0; this.size = 0; }
  silence(on) {
    this.silenced = on;
    if (on) this.clear();
  }
  quiet(samples) { return this.silentRun >= Math.max(samples, this.size); }
  process(input, output) {
    output.fill(0);
    if (this.silenced) this.silentRun += input.length;
    else for (const sample of input) {
      if (this.size === this.samples.length) throw new Error('Live playback buffer overflow');
      this.samples[(this.read + this.size++) % this.samples.length] = sample;
      this.silentRun = Math.abs(sample) > SILENT ? 0 : this.silentRun + 1;
    }
    while (this.size > this.delay + output.length) {
      let silent = true;
      for (let i = 0; i < output.length; i++) {
        if (Math.abs(this.samples[(this.read + i) % this.samples.length]) > SILENT) { silent = false; break; }
      }
      if (!silent) break;
      this.read = (this.read + output.length) % this.samples.length;
      this.size -= output.length;
    }
    for (let i = 0; i < output.length && this.size > this.delay; i++) {
      output[i] = this.samples[this.read];
      this.read = (this.read + 1) % this.samples.length;
      this.size--;
    }
  }
}
