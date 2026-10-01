export class Microphone {
  constructor({ track, ended }) {
    this.context = new AudioContext();
    this.output = this.context.createMediaStreamDestination();
    if (this.context.state === 'suspended') for (const type of ['pointerdown', 'keydown']) addEventListener(type, () => {
      if (this.context.state === 'suspended') this.context.resume();
    }, { once: true });
    this.track = track;
    this.ended = ended;
  }
  async open(capture) {
    this.context.resume().catch(() => {});
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
    this.release();
    this.output.stream.getTracks().forEach(track => track.stop());
    this.context.close();
  }
}
