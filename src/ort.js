import { env } from 'onnxruntime-web';
import wasm from '../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.jsep.wasm';

env.wasm.wasmPaths = { wasm: new URL(wasm, import.meta.url).href };

export * from 'onnxruntime-web';
