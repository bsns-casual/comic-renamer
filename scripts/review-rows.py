#!/usr/bin/env python3
"""Builds the Humble review page's data from `cargo xtask seed`'s output.

Run `cargo xtask seed` first, then:

    python3 scripts/review-rows.py <out.json>

This reads two files and judges nothing. `target/seed/humble.tsv` holds the
decomposition and `target/seed/humble-classes.tsv` holds the class and the
verified title per row, both written by `seed`.

An earlier version re-derived the classification here, and the two
implementations disagreed: this file's volume-word list carried `book`, `part`
and `v`, which `title::is_volume_word` does not match, so the page flagged
rows the report called clean. It also kept a Python copy of the frozen key
rule (invariant 10). A second implementation of a judgement is a second thing
to be wrong.
"""

import csv
import json
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROWS = ROOT / "target/seed/humble.tsv"
CLASSES = ROOT / "target/seed/humble-classes.tsv"

# Classes that mean a human has to look.
#
# The two derived-issue classes are in here on purpose. A filename is evidence
# and a sibling of the same title is evidence, but neither is a keystroke
# (D50, D51) — and before this they left the reviewable pile entirely, taking
# with them rows the old `bare trailing number` class would have caught. One
# of the 59 is wrong in a way only a reader can see: `"Shrine of the Monkey
# God!" in Kramers Ergot 9` is one story inside the ninth book of a numbered
# anthology, and the rule makes the quoted story the series and 9 the issue.
# Every derived row arrives with a better proposal than before; none arrives
# confirmed.
NEEDS_A_DECISION = {
    "bare trailing number (not a marker)",
    "volume marker swallowed into the subtitle (colon came first)",
    "multiple colons (series may contain a colon)",
    "issue taken from the filename (D61)",
    "issue copied from a sibling of the same title",
    "siblings of one title decomposed differently",
    "series is all caps (rarely right)",
}
# Which authority decided an issue, for the chip on the card.
ISSUE_SOURCE = {
    "issue written in the verified title (#N)": "title",
    "issue taken from the filename (D61)": "filename",
    "issue copied from a sibling of the same title": "sibling",
}
CORPUS_CONFIRMED = "corpus confirms the series (colon kept, volume recovered)"


def read_tsv(path, columns):
    with path.open(encoding="utf-8") as handle:
        rows = csv.reader(handle, delimiter="\t")
        header = next(rows)
        if header != columns:
            sys.exit(f"{path} has columns {header}; this build expects {columns}")
        return list(rows)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    for path in (ROWS, CLASSES):
        if not path.exists():
            sys.exit(f"{path} is missing; run `cargo xtask seed` first")

    # The sidecar is written before de-duplication, so it can hold more rows
    # than the table. Keyed by identity, which is what both files agree on.
    classes = {
        (key, bundle): (stem, title, name)
        for key, bundle, stem, title, name in read_tsv(
            CLASSES, ["key", "bundle", "stem", "title", "class"]
        )
    }

    rows, tally = [], Counter()
    table = read_tsv(
        ROWS, ["key", "series", "volume", "issue", "subtitle", "lang", "volume_count", "bundle"]
    )
    for index, record in enumerate(table):
        key, series, volume, issue, subtitle, _lang, _count, bundle = record
        found = classes.get((key, bundle))
        if found is None:
            sys.exit(f"row {index} is in the table but not the classes file: {key!r} / {bundle!r}")
        stem, title, name = found
        tally[name] += 1
        rows.append(
            {
                "i": index,
                "k": key,
                "b": bundle,
                "stem": stem,
                "title": title,
                "s": series,
                "v": volume,
                "iss": issue,
                "sub": subtitle,
                # `seed`'s own class, never re-derived. A list because the
                # page renders one chip per entry.
                "flags": [name] if name in NEEDS_A_DECISION else [],
                "src": ISSUE_SOURCE.get(name, ""),
                "corpus": name == CORPUS_CONFIRMED,
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
    for name, count in tally.most_common():
        print(f"  {count:5}  {name}")


if __name__ == "__main__":
    main()
