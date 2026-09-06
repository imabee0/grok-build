#!/usr/bin/env python3
"""Apply tools/rebrand.toml to the working tree: file contents and path names.

Idempotent: running it twice is a no-op, because every rule maps upstream
identifiers to bcode ones and no rule maps a bcode identifier back.

Regenerated from `vendor` on every upstream sync, so it must never depend on
state left behind by a previous run.
"""

from __future__ import annotations

import argparse
import os
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_rules():
    with open(ROOT / "tools" / "rebrand.toml", "rb") as fh:
        cfg = tomllib.load(fh)
    rules = [(r["from"], r["to"]) for r in cfg["rule"]]
    return rules, cfg["skip_paths"], cfg["binary_exts"], cfg.get("skip_content", [])


def apply(text: str, rules: list[tuple[str, str]]) -> str:
    for src, dst in rules:
        if src in text:
            text = text.replace(src, dst)
    return text


def skipped(rel: str, skip_paths: list[str]) -> bool:
    return any(rel == s or rel.startswith(s) for s in skip_paths)


def walk(skip_paths: list[str]):
    """Yield repo-relative paths, pruning skipped directories as we go."""
    for dirpath, dirnames, filenames in os.walk(ROOT):
        rel_dir = os.path.relpath(dirpath, ROOT)
        rel_dir = "" if rel_dir == "." else rel_dir + "/"
        dirnames[:] = [d for d in dirnames if not skipped(f"{rel_dir}{d}/", skip_paths)]
        for name in filenames:
            rel = f"{rel_dir}{name}"
            if not skipped(rel, skip_paths):
                yield rel


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true",
                    help="report what would change; exit 1 if anything would")
    args = ap.parse_args()

    rules, skip_paths, binary_exts, skip_content = load_rules()

    files = sorted(walk(skip_paths))
    content_changed = 0
    renamed = 0

    # Pass 1: file contents.
    for rel in files:
        path = ROOT / rel
        if path.suffix.lower() in binary_exts or path.is_symlink():
            continue
        if skipped(rel, skip_content):
            continue  # path still gets renamed below; only its contents are data
        try:
            raw = path.read_bytes()
        except OSError:
            continue
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            continue  # binary by content
        new = apply(text, rules)
        if new != text:
            content_changed += 1
            if not args.check:
                path.write_text(new, encoding="utf-8")

    # Pass 2: path names, deepest first so children move before their parents.
    for rel in sorted(files, key=lambda p: p.count("/"), reverse=True):
        src = ROOT / rel
        if not src.exists():
            continue  # already moved with a renamed parent
        new_rel = apply(rel, rules)
        if new_rel == rel:
            continue
        renamed += 1
        if not args.check:
            dst = ROOT / new_rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            src.rename(dst)

    # Prune directories emptied by the renames. os.walk caches its dirnames
    # list, so re-read each directory instead of trusting it: with topdown=False
    # a parent is visited after children it no longer contains.
    if not args.check:
        for dirpath, _dirnames, _filenames in os.walk(ROOT, topdown=False):
            rel = os.path.relpath(dirpath, ROOT)
            if rel == "." or skipped(rel + "/", skip_paths):
                continue
            if not os.listdir(dirpath):
                os.rmdir(dirpath)

    verb = "would change" if args.check else "changed"
    print(f"rebrand: {verb} {content_changed} file contents, {renamed} paths")
    return 1 if (args.check and (content_changed or renamed)) else 0


if __name__ == "__main__":
    sys.exit(main())
