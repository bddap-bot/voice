export class LivePlayback {
  constructor(onTranscript, onError) {
    this.onTranscript = onTranscript;
    this.onError = onError;
    this.holds = new Set();
    this.transcripts = [];
    this.closed = false;
  }
  async attach(stream) {
    const context = this.context = new AudioContext();
    await context.audioWorklet.addModule(new URL('./playback-worklet.js', import.meta.url));
    if (this.closed) return null;
    this.node = new AudioWorkletNode(context, 'live-playback', { outputChannelCount: [1], channelCount: 1, channelCountMode: 'explicit' });
    this.node.port.onmessage = ({ data }) => { if (data.error) this.onError(new Error(data.error)); };
    // Chromium needs a playing media element to decode remote WebRTC audio for Web Audio.
    this.sink = new Audio();
    this.sink.muted = true;
    this.sink.srcObject = stream;
    await this.sink.play();
    if (this.closed) return null;
    this.source = context.createMediaStreamSource(stream);
    const destination = context.createMediaStreamDestination();
    this.source.connect(this.node).connect(destination);
    this.node.port.postMessage({ type: 'hold', held: this.holds.size > 0 });
    await context.resume();
    return this.closed ? null : destination.stream;
  }
  hold(id) {
    if (this.closed) return;
    this.holds.add(id);
    this.node?.port.postMessage({ type: 'hold', held: true });
  }
  release(id) {
    if (this.closed || !this.holds.delete(id) || this.holds.size) return;
    this.node?.port.postMessage({ type: 'hold', held: false });
    for (const event of this.transcripts.splice(0)) this.onTranscript(event);
  }
  transcript(event) {
    if (this.closed) return;
    if (this.holds.size) this.transcripts.push(event);
    else this.onTranscript(event);
  }
  interrupt() {
    this.transcripts.length = 0;
    this.node?.port.postMessage({ type: 'clear' });
  }
  async close() {
    this.closed = true;
    this.transcripts.length = 0;
    this.holds.clear();
    if (this.sink) { this.sink.pause(); this.sink.srcObject = null; }
    this.source?.disconnect();
    this.node?.disconnect();
    await this.context?.close().catch(() => {});
  }
}
