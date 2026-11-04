// node web/check-avif.mjs: checks that the browser's AVIF build writes the command line's files for
// two icons, their bytes pinned here (as tests/avif.rs pins the command line's). build.sh runs it last,
// so a build that would write other files doesn't ship: libaom's SIMD reaches wasm through SIMDe, and
// an older compiler (Amazon Linux's clang 15) built it into code that writes different files; and
// libaom's workers run one after another here, which the 256 × 256 icon has several of (the 72 × 72
// one, a single row of blocks, has one). A change that alters slimdeer's AVIF output on purpose
// updates EXPECTED, from `slimdeer ICON -o OUT.avif`.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const here = new URL(".", import.meta.url);
const EXPECTED = {
  "icons/twemoji-1f600.png": "e026328870ce5abeec225772cafd6707fd626e3a82fdfebb79c0035ef78dd11b", // 1284 bytes
  "3d/fluent-light-bulb.png": "dec2206e25c5930e5588d9570b68b02650e1cef954041003eea1aecaa4ba9552", // 2240 bytes
};

const mod = await import(new URL("pkg-avif/slimdeer_wasm.js", here));
await mod.default({ module_or_path: readFileSync(new URL("pkg-avif/slimdeer_wasm_bg.wasm", here)) });
for (const [icon, expected] of Object.entries(EXPECTED)) {
  const file = new URL(`../bench/images/${icon}`, here);
  const result = mod.compress_avif(readFileSync(file), undefined);
  const digest = createHash("sha256").update(result.avif).digest("hex");
  if (digest !== expected) {
    console.error(`web/pkg-avif writes ${result.avif.length} bytes (sha256 ${digest}) for ${file.pathname}, not the command line's file`);
    process.exit(1);
  }
}
console.log("web/pkg-avif writes the command line's files");
