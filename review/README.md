# The reviewed Humble table

`humble-decisions.tsv` is the Humble bundle table after a human decided every
row: 2,199 rows, 2,031 distinct keys, 680 series. `humble-decisions-not-comics.tsv`
holds the five files the reviewer marked as not comics — two light novels and
three audiobooks — so a later `seed` run has a record of them instead of
proposing them again.

It is **not** the shipped dataset. `data/decisions.tsv` is, and it is
`include_str!`-embedded in the binary, which is a deliberate act of shipping.
This directory is the reviewed data holding its own copy outside that decision.

## Why it lives here and not in `data/decisions.tsv`

Publishing Humble-derived titles as the product's dataset is an open question
the owner deferred: `data/LICENSE` ties rule data to the Grand Comics Database
under CC BY-SA 4.0, these titles came from Humble's order API, and D31
committed them as *test* data. That question gates publishing. It does not gate
keeping a copy, and the alternative was 2,204 human judgements living only in
one web page's database.

## How it was made

```
python3 scripts/export-decisions.py <dump-dir> review/humble-decisions.tsv
```

`<dump-dir>` is the review artifact's `decisions` collection, dumped one JSON
file per block of 100. The exporter judges nothing and never recomputes a key —
the frozen key rule (invariant 10) has one implementation, in Rust.

## It is validated, and how

Checked on 2026-10-27 by temporarily copying it to `data/mappings/humble.tsv`
and adding it to `SHIPPED_FILES`, then running the real gates rather than a
second implementation of them. **That check predates D63**, which merged the
per-marketplace files into one table on 2026-10-30; the gates it names still
exist and still run, but the mechanics below have changed:

- `cargo test --workspace` — **606 passed, 0 failed.** That covers
  `MappingRow::validate` on every row (§6.11's forbid-list and 120-byte cap),
  `find_key_collisions` over the joined table including the 12 marketplace rows
  already shipped, and the frozen key pin.
- `cargo xtask check-mappings` — file size within the cap, now 4 MiB (D67).
  The file is 205 KB.

Both changes were reverted. The row cap `check-mappings` enforces is 2,500 per
**pull request**, and this repo's convention is to push to `main`, so that cap
is not what is holding this file here — the licence question is.

## To ship it, once that question is answered

Simpler than it was, because D63 left one embedded file rather than an array,
and harder in one way, because D64 made the key the whole identity:

1. **Drop the `bundle` column** (field 8). The shipped table has seven
   columns; `parse_tsv` refuses eight and says so.
2. **Collapse to one row per key.** 2,199 rows hold 2,031 distinct keys, so
   168 are repeats — every one of them agreeing on every field, which is the
   measurement D64 rests on. `find_key_collisions` refuses a repeated key, so
   this is not optional.
3. Append the result to `data/decisions.tsv` and `git rm` the file here.
4. `cargo test --workspace`, which is what proves steps 1 and 2 were done:
   `the_decisions_file_is_the_one_that_ships` checks every row on disk is a row
   the binary holds, and the loader refuses a duplicate key.

No `include_str!` edit and no array size — there is one of each now.
