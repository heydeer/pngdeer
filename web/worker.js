// Runs pngdeer's WebAssembly compressor off the main thread, one file at a time. On a cross-origin
// isolated page (see serve.py) it loads the threaded build, whose rayon pool spreads each file's
// quantizing, scoring and oxipng over all cores; elsewhere, or with ?single, the single-threaded one.
const threaded = self.crossOriginIsolated === true && !new URL(self.location.href).searchParams.has("single");

// 16×16 gradient. Compressing it first proves the thread pool works: in Chromium, a page reloaded
// while threads were running can start new ones that never pick up work, so compression would hang
// forever. The page waits for "ready" and falls back to single-threaded workers if it doesn't come.
const SELF_TEST = Uint8Array.from(atob(
  "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAACD0lEQVR42g3LoQFEEQAAUCMYwQhGMIIRjGAE7SJNpIk0kSbSRJpIE91//QEAHgQXgY3BImBQ0BmoHBQBkgLRAh+Ay8A0oCeQB/wAgA/Ci+DGcBE4KOwMVg6LgEnBaKEP0GVoGtQTygO/gB5EF6GN0SJoUNQZqhwVgZJC0SIfkMvINKQnkgd9AT+IL8Ib40XwoLgzXDkuAieFo8U+YJexaVhPLA/+AnmQXEQ2JouQQUlnpHJSBEmKREt8IC4T04ieRB7yBfogvYhuTBehg9LOaOW0CJoUjZb6QF2mplE9qTz0C+xBdhHbmC3CBmWdscpZESwpFi3zgbnMTGN6MnnYF/iD/CK+MV+ED8o745XzInhSPFruA3eZm8b15PLwL4gHxUViY7GIGFR0JioXRYikRLTCB+GyME3oKeQRX1APqovUxmoRNajqTFWuilBJqWiVD8plZZrSU8mjvmAftBfZje0idlDbma3cFmGTstFaH6zL1jSrp5XHfiE8GC4KG4dFwqChs1B5KCIkFaINPgSXg2lBzyBP+EJ+MF+UN86L5EFzZ7nyXEROKkebfcguZ9Oynlme/IX2YLuobdwWaYO2zlrlrYiWVIu2+dBcbqY1PZs87QvzwXnR3HguMgednc3KZxEzqRnt9GG6PE2bek555hfOg+eis/FZ5Ax6OjuVnyJOUifa48Nx+Zh29DzynN8f4RqzEK8Frl4AAAAASUVORK5CYII="
), (c) => c.charCodeAt(0));

// Peak memory is ~260 bytes a pixel and wasm32 has 4 GB. Running out of memory on a thread of the
// threaded build hangs it, so it takes images up to MT_MAX_PIXELS and the single-threaded build
// (which fails cleanly) the rest, up to MAX_PIXELS.
const MT_MAX_PIXELS = 12e6;
const MAX_PIXELS = 16e6;
let single = null; // the single-threaded build, loaded when a big image needs it in a threaded worker

const ready = (async () => {
  const mod = await import(threaded ? "./pkg-mt/pngdeer_wasm.js" : "./pkg/pngdeer_wasm.js");
  await mod.default();
  if (threaded) {
    await mod.initThreadPool(navigator.hardwareConcurrency);
    mod.compress(SELF_TEST, 85, 0).free();
  }
  self.postMessage({ type: "ready", threaded });
  return mod;
})();

self.onmessage = async ({ data }) => {
  const { id, bytes, target, preview } = data; // no target: automatic quality; no preview: pngdeer's default
  try {
    const input = new Uint8Array(bytes);
    const pixels = new DataView(input.buffer).getUint32(16) * new DataView(input.buffer).getUint32(20); // IHDR
    if (pixels > MAX_PIXELS) {
      throw new Error(`too large for the browser version (${(pixels / 1e6).toFixed(1)} MP, at most ${MAX_PIXELS / 1e6} MP)`);
    }
    let { compress } = await ready;
    if (threaded && pixels > MT_MAX_PIXELS) {
      single ??= import("./pkg/pngdeer_wasm.js").then(async (mod) => (await mod.default(), mod));
      ({ compress } = await single);
    }
    const start = performance.now();
    const result = compress(input, target, preview);
    const ms = performance.now() - start;
    const png = result.png;
    const report = JSON.parse(result.report);
    result.free();
    self.postMessage({ id, png, report, ms, threaded }, [png.buffer]);
  } catch (error) {
    self.postMessage({ id, error: String(error) });
  }
};
