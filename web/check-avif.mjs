// node web/check-avif.mjs: checks that the browser's AVIF build writes the command line's file for one
// icon, its bytes pinned here (as tests/avif.rs pins the command line's). build.sh runs it last, so a
// build that would write other files doesn't ship: libaom's SIMD reaches wasm through SIMDe, and an
// older compiler (Amazon Linux's clang 15) built it into code that writes different files. A change
// that alters pngdeer's AVIF output on purpose updates EXPECTED, from `pngdeer ICON -o OUT.avif`.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const here = new URL(".", import.meta.url);
const ICON = new URL("../bench/images/icons/twemoji-1f600.png", here);
const EXPECTED = "e026328870ce5abeec225772cafd6707fd626e3a82fdfebb79c0035ef78dd11b"; // 1284 bytes

const mod = await import(new URL("pkg-avif/pngdeer_wasm.js", here));
await mod.default({ module_or_path: readFileSync(new URL("pkg-avif/pngdeer_wasm_bg.wasm", here)) });
const result = mod.compress_avif(readFileSync(ICON), undefined);
const digest = createHash("sha256").update(result.avif).digest("hex");
if (digest !== EXPECTED) {
  console.error(`web/pkg-avif writes ${result.avif.length} bytes (sha256 ${digest}) for ${ICON.pathname}, not the command line's file`);
  process.exit(1);
}
console.log("web/pkg-avif writes the command line's file");
