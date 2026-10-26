#!/usr/bin/env python3
"""Builds the Humble review page's data from `cargo xtask seed`'s output.

Run `cargo xtask seed` first; this reads `target/seed/humble.tsv` and joins it
to `fixtures/humble-ground-truth.tsv` for the verified title each row was
built from. The decomposition is never recomputed here — `seed` owns it, and a
second implementation of the split is a second thing to be wrong.

    python3 scripts/review-rows.py <out.json>

Flags mark rows whose split disagrees with the verified title. They are a
review aid, not a gate: a wrong split costs the user a correction.
"""

import csv
import json
import re
import sys
import unicodedata
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "target/seed/humble.tsv"
TRUTH = ROOT / "fixtures/humble-ground-truth.tsv"
VOLUME_WORDS = {"vol", "vols", "vol.", "volume", "volumes", "v", "book", "part"}


def key(text):
    """The frozen key rule: NFC, lowercase, strip non-alphanumerics."""
    folded = unicodedata.normalize("NFC", text).lower()
    return "".join(c for c in folded if c.isalnum())


def verified_titles():
    titles = {}
    with TRUTH.open(encoding="utf-8") as handle:
        rows = csv.reader(handle, delimiter="\t")
        next(rows)
        for row in rows:
            if len(row) >= 3:
                titles[(key(row[2]), row[0])] = (row[1], row[2])
    return titles


def flags_for(title, volume, issue, subtitle):
    """Why this row wants a human. Empty means no heuristic fired, which is
    not the same as being right."""
    found = []
    if title.count(":") > 1:
        found.append("two colons")
    if volume or issue:
        return found
    words = subtitle.split()
    if any(
        words[i].lower() in VOLUME_WORDS and re.fullmatch(r"\d+", words[i + 1])
        for i in range(len(words) - 1)
    ):
        found.append("marker in subtitle")
    if title.split() and title.split()[-1].isdigit():
        found.append("bare trailing number")
    return found


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    if not SEED.exists():
        sys.exit(f"{SEED} is missing; run `cargo xtask seed` first")

    titles = verified_titles()
    rows, tally = [], Counter()
    with SEED.open(encoding="utf-8") as handle:
        reader = csv.reader(handle, delimiter="\t")
        header = next(reader)
        want = ["key", "series", "volume", "issue", "subtitle", "lang", "volume_count", "bundle"]
        if header != want:
            sys.exit(f"unexpected seed columns {header}; this build expects {want}")
        for index, record in enumerate(reader):
            row_key, series, volume, issue, subtitle, _lang, _count, bundle = record
            found = titles.get((row_key, bundle))
            if found is None:
                sys.exit(f"row {index} has no verified title: {row_key!r} / {bundle!r}")
            title, stem = found
            flags = flags_for(title, volume, issue, subtitle)
            tally["flagged" if flags else "clean"] += 1
            rows.append(
                {
                    "i": index,
                    "k": row_key,
                    "b": bundle,
                    "stem": stem,
                    "title": title,
                    "s": series,
                    "v": volume,
                    "iss": issue,
                    "sub": subtitle,
                    "flags": flags,
                    # Which authority decided the issue, where one did. Shown
                    # as a chip so a lookup is distinguishable from a guess.
                    "src": ("title" if issue and "#" in title else "filename" if issue else ""),
                    "corpus": bool(volume) and ":" in series,
                }
            )

    suspect = sorted((r for r in rows if r["flags"]), key=lambda r: r["stem"].lower())
    scan = sorted((r for r in rows if not r["flags"]), key=lambda r: r["stem"].lower())
    out = {"chunk": 100, "total": len(rows), "suspect": suspect, "scan": scan}
    Path(sys.argv[1]).write_text(
        json.dumps(out, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )

    print(f"{len(rows)} rows -> {sys.argv[1]}")
    print(f"  {len(suspect)} need a decision, {len(scan)} to scan")
    for name, count in [
        ("issue written in the title", sum(1 for r in rows if r["src"] == "title")),
        ("issue from the filename", sum(1 for r in rows if r["src"] == "filename")),
        ("corpus confirmed the series", sum(1 for r in rows if r["corpus"])),
    ]:
        print(f"  {count:5}  {name}")


if __name__ == "__main__":
    main()
