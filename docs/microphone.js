export class Microphone {
  constructor({ track, ended, state }) {
    this.context = new AudioContext();
    this.output = this.context.createMediaStreamDestination();
    this.resume = () => {
      if (this.context.state !== 'running' && this.context.state !== 'closed') this.context.resume().catch(() => {});
    };
    for (const type of ['pointerdown', 'keydown']) addEventListener(type, this.resume);
    this.context.addEventListener('statechange', () => {
      this.state(this.context.state);
      this.resume();
    });
    this.track = track;
    this.ended = ended;
    this.state = state;
  }
  running() {
    return new Promise(resolve => {
      const settle = () => {
        if (this.context.state !== 'running' && this.context.state !== 'closed') return;
        this.context.removeEventListener('statechange', settle);
        resolve();
      };
      this.context.addEventListener('statechange', settle);
      settle();
      if (this.context.state !== 'running') this.state(this.context.state);
      this.resume();
    });
  }
  async open(capture) {
    if (capture() && !this.opening) {
      const opening = this.opening = navigator.mediaDevices.getUserMedia({ audio: true }).then(stream => {
        if (this.opening !== opening) {
          stream.getTracks().forEach(track => track.stop());
          return;
        }
        this.input = stream;
        this.source = this.context.createMediaStreamSource(stream);
        this.source.connect(this.output);
        for (const track of stream.getAudioTracks()) {
          this.track(track);
          track.addEventListener('ended', () => {
            if (this.input === stream) this.ended(this.output.stream);
          });
        }
      }, error => {
        if (this.opening !== opening) return;
        this.opening = undefined;
        throw error;
      });
    }
    await this.opening;
    await this.running();
    return this.output.stream;
  }
  release() {
    this.opening = undefined;
    this.source?.disconnect();
    this.source = undefined;
    this.input?.getTracks().forEach(track => track.stop());
    this.input = undefined;
  }
  close() {
    for (const type of ['pointerdown', 'keydown']) removeEventListener(type, this.resume);
    this.release();
    this.output.stream.getTracks().forEach(track => track.stop());
    this.context.close();
  }
}
