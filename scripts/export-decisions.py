#!/usr/bin/env python3
"""Turns the review page's decisions into the TSV `seed` writes.

    python3 scripts/export-decisions.py <dump-dir> review/humble-decisions.tsv

`<dump-dir>` holds one JSON file per block of 100 decisions, exactly as the
artifact's `decisions` collection stores them: `{"d": {"<ordinal>": {...}}}`.
Dump it with the Claude artifact database tool, `action: "list"`, the
`decisions` collection, and an `out_dir`.

This exists because the review ran in a web page and its decisions lived only
in that page's store. 2,204 human judgements with no copy in the repository is
a worse risk than the licence question that gates publishing them.

**It judges nothing and it does not recompute a key.** The frozen key rule
(invariant 10) has exactly one implementation, in Rust, and `review-rows.py`
already had to delete a Python copy of it. The key in a decision was written
from the row `seed` produced, so it is carried across untouched. A decision
also carries its row's bundle; both are the row's identity and neither is
derived here.

Rows the reviewer marked `skip` are not comics and are written to a separate
file beside the output, so a later `seed` run has a record of them rather than
proposing them again.
"""

import csv
import json
import pathlib
import sys

# `seed`'s own column order, which `review-rows.py` also asserts against.
COLUMNS = ["key", "series", "volume", "issue", "subtitle", "lang", "volume_count", "bundle"]
# The language every Humble row carries; `seed`'s LANG.
LANG = "en"


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    dump, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    blocks = sorted(dump.glob("*.json"))
    if not blocks:
        sys.exit(f"{dump} holds no JSON blocks; dump the `decisions` collection into it first")

    decisions = {}
    for block in blocks:
        body = json.loads(block.read_text(encoding="utf-8"))
        for at, decision in body.get("d", {}).items():
            at = int(at)
            if at in decisions:
                sys.exit(f"ordinal {at} appears in two blocks; the dump is inconsistent")
            decisions[at] = decision

    rows, skipped = [], []
    for at in sorted(decisions):
        d = decisions[at]
        key, bundle = d.get("k"), d.get("b")
        if not key or not bundle:
            sys.exit(f"decision {at} carries no key or bundle: {d!r}")
        if d.get("skip"):
            skipped.append([key, bundle])
            continue
        series = (d.get("s") or "").strip()
        if not series:
            sys.exit(f"decision {at} has no series, so its row has no destination: {d!r}")
        for field in ("v", "iss"):
            value = (d.get(field) or "").strip()
            if value and not value.isdigit():
                sys.exit(f"decision {at} has a non-numeric {field}: {value!r}")
        rows.append([
            key,
            series,
            (d.get("v") or "").strip(),
            (d.get("iss") or "").strip(),
            (d.get("sub") or "").strip(),
            LANG,
            "",  # volume_count is the owner's to set per series (D44).
            bundle,
        ])

    # One key must describe one comic. `find_key_collisions` refuses a pair
    # that disagrees, and it is cheaper to say so here, naming the key, than to
    # read it out of a failed build.
    fields = {}
    for row in rows:
        key, rest = row[0], tuple(row[1:5])
        if key in fields and fields[key] != rest:
            sys.exit(f"key {key!r} has two decompositions: {fields[key]} and {rest}")
        fields[key] = rest

    # Sorted so a regenerated file is a stable diff rather than a reshuffle.
    rows.sort(key=lambda r: (r[0], r[7]))
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8", newline="\n") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n", quoting=csv.QUOTE_NONE)
        writer.writerow(COLUMNS)
        writer.writerows(rows)

    aside = out.with_name(out.stem + "-not-comics.tsv")
    skipped.sort()
    with aside.open("w", encoding="utf-8", newline="\n") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n", quoting=csv.QUOTE_NONE)
        writer.writerow(["key", "bundle"])
        writer.writerows(skipped)

    print(f"{len(decisions)} decisions in {len(blocks)} blocks")
    print(f"  {len(rows)} rows -> {out}")
    print(f"  {len(skipped)} marked not a comic -> {aside}")
    print(f"  {len(fields)} distinct keys, {len({r[1] for r in rows})} distinct series")


if __name__ == "__main__":
    main()
