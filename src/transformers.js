// transformers points onnxruntime at a CDN unless wasmPaths is already set when its module body runs.
import './ort.js';

export { env, pipeline } from '@huggingface/transformers';
