import { SpeechSegmenter } from './wake.js';

class WakeCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.segmenter = new SpeechSegmenter(sampleRate);
  }
  process([input]) {
    for (const segment of this.segmenter.push(input[0] ?? [])) this.port.postMessage(segment, [segment.buffer]);
    return true;
  }
}
registerProcessor('wake-capture', WakeCapture);
