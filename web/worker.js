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

// Large images peak at about 80 bytes a pixel, 87 with transparency (above 8 MP pngdeer keeps a lean
// reference: LEAN_PIXELS in src/search.rs), in either build, and wasm32 has 4 GB. Running out of
// memory would hang the threaded build, so the limit leaves room: 40 MP peaks at about 3.5 GB.
const MAX_PIXELS = 40e6;

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
    const ihdr = new DataView(input.buffer, 16, 8);
    const [width, height] = [ihdr.getUint32(0), ihdr.getUint32(4)];
    const pixels = width * height;
    if (pixels > MAX_PIXELS) {
      // in pixels as well as megapixels: a well-compressed screenshot can be a few MB and still too big
      const size = `${width} × ${height} pixels (${(pixels / 1e6).toFixed(1)} MP)`;
      throw new Error(`too large for the browser version: ${size}, at most ${MAX_PIXELS / 1e6} MP; use the command line`);
    }
    const { compress } = await ready;
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
