export class LivePlayback {
  constructor(onError, speaker = null) {
    this.onError = onError;
    this.speaker = speaker;
    this.quietWaiters = [];
    this.closed = false;
  }
  async attach(stream) {
    const context = this.context = new AudioContext();
    await context.audioWorklet.addModule(new URL('./playback-worklet.js', import.meta.url));
    if (this.closed) return null;
    this.node = new AudioWorkletNode(context, 'live-playback', { outputChannelCount: [1], channelCount: 1, channelCountMode: 'explicit' });
    this.node.port.onmessage = ({ data }) => {
      if (data.error) this.onError(new Error(data.error));
      if (data.quiet) for (const resolve of this.quietWaiters.splice(0)) resolve();
    };
    this.sink = new Audio();
    this.sink.muted = true;
    this.sink.srcObject = stream;
    await this.sink.play();
    if (this.closed) return null;
    this.source = context.createMediaStreamSource(stream);
    const destination = context.createMediaStreamDestination();
    this.source.connect(this.node).connect(destination);
    this.outputStream = destination.stream;
    this.lifecycle = new AbortController();
    const options = { signal: this.lifecycle.signal };
    const recover = () => this.recover();
    document.addEventListener('visibilitychange', recover, options);
    for (const event of ['focus', 'pageshow', 'pointerdown', 'keydown']) window.addEventListener(event, recover, options);
    await context.resume();
    return this.closed ? null : this.outputStream;
  }
  recover() {
    if (this.closed || document.hidden || !this.context) return;
    // Resume each stage together: awaiting resume first can lose user activation.
    const pending = [];
    if (this.context.state !== 'running') pending.push(this.context.resume());
    if (this.sink?.paused) pending.push(this.sink.play());
    if (this.speaker?.srcObject?.id === this.outputStream.id && this.speaker.paused) pending.push(this.speaker.play());
    Promise.all(pending).catch((error) => {
      // A browser may require a gesture after an interruption. Keep the session
      // and retry on the next pointer/key event instead of discarding its audio.
      if (!this.closed && error.name !== 'NotAllowedError' && error.name !== 'AbortError') this.onError(error);
    });
  }
  interrupt() {
    this.node?.port.postMessage({ type: 'clear' });
  }
  quiet(ms = 2000) {
    if (!this.node) return Promise.resolve();
    return new Promise((resolve) => {
      this.quietWaiters.push(resolve);
      this.node.port.postMessage({ type: 'quiet', ms });
    });
  }
  async close() {
    this.closed = true;
    for (const resolve of this.quietWaiters.splice(0)) resolve();
    this.lifecycle?.abort();
    if (this.sink) { this.sink.pause(); this.sink.srcObject = null; }
    this.source?.disconnect();
    this.node?.disconnect();
    await this.context?.close().catch(() => {});
  }
}
