#!/usr/bin/env python3
"""Writes web/THIRD_PARTY_LICENSES.txt: the license of every crate (and libdeflate's C code) compiled
into the browser build, which ships them all in one .wasm file. Run after changing dependencies:

  python3 web/notices.py
"""

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMES = ("license", "licence", "copying", "notice", "copyright", "patents")


def license_files(directory: Path) -> list[Path]:
    """A crate's license files; one directory down too for -sys crates (vendored C code) or when the
    top has none."""
    found = sorted(p for p in directory.iterdir() if p.is_file() and p.name.lower().startswith(NAMES))
    if not found or directory.name.split("-")[-2:-1] == ["sys"]:
        found += sorted(p for p in directory.glob("*/*") if p.is_file() and p.name.lower().startswith(NAMES))
    return found


def main() -> None:
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "wasm32-unknown-unknown", "--features", "pngdeer-wasm/threads"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    # proc-macro crates (and what only they use) run at compile time and aren't in the .wasm
    is_macro = lambda i: any("proc-macro" in t["kind"] for t in packages[i]["targets"])
    root = next(i for i, p in packages.items() if p["name"] == "pngdeer-wasm")
    seen, stack = set(), [root]
    while stack:
        node = stack.pop()
        if node in seen or is_macro(node):
            continue
        seen.add(node)
        stack += [d["pkg"] for d in nodes[node]["deps"] if any(k["kind"] is None for k in d["dep_kinds"])]
    texts: dict[str, tuple[str, list[str]]] = {}  # text with whitespace collapsed -> (text, crates under it)
    unfiled = []
    for p in sorted((packages[i] for i in seen if packages[i]["name"] not in ("pngdeer", "pngdeer-wasm")), key=lambda p: (p["name"], p["version"])):
        label = f"{p['name']} {p['version']} ({p.get('license') or 'see its source'})"
        files = license_files(Path(p["manifest_path"]).parent)
        if not files:
            unfiled.append(label)
        for f in files:
            text = f.read_text(errors="replace").strip()
            texts.setdefault(" ".join(text.split()), (text, []))[1].append(label)
    parts = ["Third-party software compiled into pngdeer's browser build (web/pkg, web/pkg-mt), and its licenses.",
             "Each license text below is followed by the crates it covers.", ""]
    for text, crates in sorted(texts.values(), key=lambda v: v[1][0]):
        parts += ["=" * 78, *crates, "-" * 78, text, ""]
    if unfiled:
        parts += ["=" * 78, "Published without a license file (license as declared):", *unfiled, ""]
    (ROOT / "web" / "THIRD_PARTY_LICENSES.txt").write_text("\n".join(parts) + "\n")

if __name__ == "__main__":
    main()
