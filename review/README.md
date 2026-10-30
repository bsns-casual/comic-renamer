# The reviewed Humble table — removed from this repository

`humble-decisions.tsv` and `humble-decisions-not-comics.tsv` **are not here any
more, and are not in this repository's history.** They were excised on
2026-10-30 by a `git filter-branch` over all 220 commits, after the owner
decided that publishing Humble-derived titles was not something he wanted the
repository to carry. Every commit SHA changed as a result; see CLAUDE.md's note
on the rewrite.

## What the table was

2,031 rows in the shipped seven-column shape — one row per key, no `bundle`
column — being the Humble bundle table after a human decided every one of
2,204 candidate rows. Plus five rows recording files the reviewer marked as not
comics, so a later `seed` run has a record of them instead of proposing them
again.

Measured before removal, and worth keeping because D64 rests on it: the 2,199
eight-column rows held 2,031 distinct keys, so 168 were repeats, and **zero of
those repeats disagreed on any of the seven fields.**

## Where it lives now

Not in git. Two places:

- **The review artifact's own store**, which is the authority. Re-dump its
  `decisions` collection and run
  `python3 scripts/export-decisions.py <dump-dir> <out.tsv>` — the exporter
  judges nothing and never recomputes a key, because the frozen key rule
  (invariant 10) has one implementation, in Rust.
- Wherever the owner kept the copy taken at removal time. That is deliberately
  not recorded here.

`scripts/export-decisions.py` still writes the **eight-column** shape with
`bundle`. The shipped table is seven columns (D64), so a regenerated export
needs the same two transformations that were applied before: drop field 8, and
collapse rows that then share a key, asserting they agree first.

## If it is ever to ship

1. Regenerate or restore it, apply those two transformations.
2. `tail -n +2 <the-file> >> data/decisions.tsv`

No `include_str!` edit and no array size — D63 left one embedded file.
`cargo test --workspace --no-fail-fast` is what proves step 1 was done right:
`MappingRow::validate` runs over every row, `find_key_collisions` refuses a
repeated key, and `the_decisions_file_is_the_one_that_ships` checks every row
on disk is a row the binary holds. Validated that way on 2026-10-30, before
removal: the joined 2,043-row table gave 616 passed, 0 failed, at 132 KB
against the 4 MiB cap.

**Do not validate by copying anything into `data/mappings/`.** That path is
gitignored and `the_decisions_file_is_the_one_that_ships` is recursive, because
three adversarial seats did exactly that and every gate stayed green.
