// node experiments/avif/chrome.mjs AVIF... — decodes each AVIF in headless Chrome (createImageBitmap, drawn on a
// canvas, read back as PNG) and writes AVIF.chrome.png next to it: the pixels a browser shows.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
const files = process.argv.slice(2);
const chrome = spawn("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", [
  "--headless=new", `--user-data-dir=${mkdtempSync(tmpdir() + "/cdp-")}`, "--remote-debugging-port=9335", "--no-first-run", "about:blank",
], { stdio: "ignore" });
const base = "http://127.0.0.1:9335";
for (let i = 0; i < 50; i++) { try { await fetch(`${base}/json/version`); break; } catch { await new Promise((r) => setTimeout(r, 200)); } }
const target = await (await fetch(`${base}/json/new?about:blank`, { method: "PUT" })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } };
const send = (method, params) => new Promise((r) => { pending.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });
process.on("exit", () => chrome.kill());
console.log("chrome", (await (await fetch(`${base}/json/version`)).json()).Browser);
for (const f of files) {
  const b64 = readFileSync(f).toString("base64");
  const r = await send("Runtime.evaluate", { awaitPromise: true, returnByValue: true, expression: `(async () => {
    const bytes = Uint8Array.from(atob("${b64}"), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/avif" }), { premultiplyAlpha: "none" });
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    canvas.getContext("2d").drawImage(bitmap, 0, 0);
    const png = new Uint8Array(await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer());
    let s = ""; for (let i = 0; i < png.length; i += 32768) s += String.fromCharCode(...png.subarray(i, i + 32768));
    return btoa(s);
  })()` });
  if (r.result?.exceptionDetails) { console.log(f, JSON.stringify(r.result.exceptionDetails).slice(0, 300)); continue; }
  writeFileSync(f + ".chrome.png", Buffer.from(r.result.result.value, "base64"));
  console.log("decoded", f);
}
process.exit(0);
