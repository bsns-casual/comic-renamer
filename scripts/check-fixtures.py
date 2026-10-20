#!/usr/bin/env python3
"""Validate the held-out filename fixtures. Exits non-zero on any problem."""
import re, sys

EXT = re.compile(r"\.(pdf|cbz|cbr|epub|cb7|cbt)$", re.I)

def main(path="fixtures/heldout-filenames.txt"):
    lines = [l.rstrip("\n") for l in open(path, encoding="utf-8")]
    lines = [l for l in lines if l.strip()]
    problems = []
    if len(lines) < 1000:
        problems.append("suspiciously small fixture set: %d entries" % len(lines))
    bad_ext = [l for l in lines if not EXT.search(l)]
    if bad_ext:
        problems.append("non-comic entries: %s" % bad_ext[:5])
    # Fixtures are bare filenames, never paths -- see CLAUDE.md invariant 9.
    paths = [l for l in lines if "/" in l or "\\" in l or l.startswith("~")]
    if paths:
        problems.append("entries must be bare filenames, not paths: %s" % paths[:5])
    for p in problems:
        print("error: %s" % p, file=sys.stderr)
    if problems:
        return 1
    print("fixtures OK: %d entries, %d distinct" % (len(lines), len(set(lines))))
    return 0

if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:]))
