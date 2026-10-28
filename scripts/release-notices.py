#!/usr/bin/env python3
"""Collect licenses for the dependencies shipped in a native pngdeer release."""

import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMES = ("license", "licence", "copying", "notice", "copyright", "patents")


def cargo(*args: str) -> str:
    return subprocess.check_output(["cargo", *args], cwd=ROOT, text=True)


def license_files(package: dict) -> list[Path]:
    directory = Path(package["manifest_path"]).parent
    files = {p for p in directory.iterdir() if p.is_file() and p.name.lower().startswith(NAMES)}
    if not files or package["name"].endswith("-sys"):
        files.update(p for p in directory.glob("*/*") if p.is_file() and p.name.lower().startswith(NAMES))
    if package.get("license_file"):
        files.add(directory / package["license_file"])
    return sorted(files)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple of the release binary")
    parser.add_argument("--output", required=True, type=Path, help="Destination notices file")
    args = parser.parse_args()

    meta = json.loads(cargo("metadata", "--locked", "--format-version", "1", "--filter-platform", args.target))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    # Metadata unifies features across the workspace, even with --filter-platform.
    # cargo tree selects only the CLI's default features, without wasm/quantlab extras.
    tree = cargo("tree", "--locked", "--package", "pngdeer", "--target", args.target,
                 "--edges", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}", "--color", "never")
    active = {(line.split()[0], line.split()[1].removeprefix("v")) for line in tree.splitlines() if line.strip()}
    key = lambda p: (p["name"], p["version"])
    seen, stack = set(), [meta["resolve"]["root"]]
    while stack:
        node = stack.pop()
        package = packages[node]
        if node in seen or key(package) not in active or any("proc-macro" in t["kind"] for t in package["targets"]):
            continue
        seen.add(node)
        stack.extend(d["pkg"] for d in nodes[node]["deps"] if any(k["kind"] is None for k in d["dep_kinds"]))
    if {key(packages[i]) for i in seen} != active:
        raise SystemExit("Could not match every runtime dependency to its package metadata")

    texts: dict[str, tuple[str, list[str]]] = {}
    unfiled = []
    dependencies = sorted((packages[i] for i in seen if packages[i]["name"] != "pngdeer"), key=key)
    for package in dependencies:
        label = f"{package['name']} {package['version']} ({package.get('license') or 'see its source'})"
        files = license_files(package)
        if not files:
            if not package.get("license"):
                raise SystemExit(f"No license found for {package['name']} {package['version']}")
            unfiled.append(label)
        for path in files:
            text = path.read_text(errors="replace").strip()
            if not text:
                raise SystemExit(f"Empty license file: {path}")
            labels = texts.setdefault(" ".join(text.split()), (text, []))[1]
            if label not in labels:
                labels.append(label)

    parts = [f"Third-party software compiled into pngdeer's native CLI ({args.target}), and its licenses.",
             "Each license text below is preceded by the crates it covers.", ""]
    for text, labels in sorted(texts.values(), key=lambda item: item[1][0]):
        parts += ["=" * 78, *labels, "-" * 78, text, ""]
    if unfiled:
        parts += ["=" * 78, "Published without a license file (license as declared):", *unfiled, ""]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(parts) + "\n", encoding="utf-8")
    print(f"Wrote notices for {len(dependencies)} dependencies to {args.output}")


if __name__ == "__main__":
    main()
