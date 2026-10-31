// node experiments/avif/wasm.mjs AVIFLAB.wasm IN.png OUT.avif Q SPEED
// rav1e in wasm (tools/aviflab built for wasm32-unknown-unknown), as `aviflab enc` encodes natively;
// prints the encode's milliseconds. IN is an 8-bit RGB or RGBA PNG (`aviflab png` writes them).
import { readFileSync, writeFileSync } from "node:fs";
const [wasmPath, input, output, quality, speed] = process.argv.slice(2);
const module = new WebAssembly.Module(readFileSync(wasmPath));
// wasm-bindgen's placeholders (a dependency links it), never called by these raw exports
const stubs = {};
for (const i of WebAssembly.Module.imports(module)) (stubs[i.module] ??= {})[i.name] = () => { throw new Error(`called ${i.name}`); };
const { exports } = new WebAssembly.Instance(module, stubs);
const png = readFileSync(input);
const ptr = exports.alloc(png.length);
new Uint8Array(exports.memory.buffer, ptr, png.length).set(png);
const start = performance.now();
const n = exports.encode_png(ptr, png.length, Number(quality), Number(speed));
console.log(Math.round(performance.now() - start));
writeFileSync(output, new Uint8Array(exports.memory.buffer, exports.out_ptr(), n));
