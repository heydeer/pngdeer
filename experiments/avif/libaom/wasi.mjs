// node experiments/avif/libaom/wasi.mjs PROGRAM.wasm ARGS...: runs a WASI preview1 program (aomenc) with the current directory preopened (pass relative paths)
import { readFileSync } from "node:fs";
import { WASI } from "node:wasi";
const [file, ...args] = process.argv.slice(2);
const wasi = new WASI({ version: "preview1", args: [file, ...args], preopens: { ".": process.cwd() }, returnOnExit: true });
const module = await WebAssembly.compile(readFileSync(file));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
