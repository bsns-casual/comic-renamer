# The reviewed Humble table

`humble-decisions.tsv` is the Humble bundle table after a human decided every
row: **2,031 rows in the shipped shape** — seven columns, one row per key.

It held 2,199 eight-column rows until 2026-10-30, when D63/D64 changed what the
shipped table is. The transformation dropped the `bundle` column and collapsed
168 repeated keys; **zero of them disagreed on any of the seven fields**, which
is the measurement D64 rests on, now confirmed on the real decided table rather
than argued. Which bundles a key appeared under — 158 keys appeared under more
than one — is in this file's git history and in the review artifact's own
store, which `scripts/export-decisions.py` can re-dump. `humble-decisions-not-comics.tsv`
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

Two steps. The data work is done:

1. `tail -n +2 review/humble-decisions.tsv >> data/decisions.tsv`
2. `git rm review/humble-decisions.tsv` (and decide what becomes of
   `humble-decisions-not-comics.tsv`, which is a record of five rejected files
   and still carries its `bundle` column, because it is not a mapping table)

No `include_str!` edit and no array size — D63 left one embedded file. No
column drop and no key collapse — done 2026-10-30.

**Checked by doing exactly step 1 and reverting**, on 2026-10-30: the joined
2,043-row table gives `cargo test --workspace --no-fail-fast` **613 passed, 0
failed** — `MappingRow::validate` over every row, `find_key_collisions` over
the whole table, the frozen key pin, and
`the_decisions_file_is_the_one_that_ships`. 132 KB against the 4 MiB cap, and
zero key overlap with the 12 rows already shipped. The 2,500-rows-per-PR cap is
PR-only and this repo pushes to `main`, so it is not what holds the file here.
The licence question is.

**Do not validate by copying this into `data/mappings/`.** That path is what an
older version of CLAUDE.md suggested; it is now gitignored, and
`the_decisions_file_is_the_one_that_ships` was made recursive on 2026-10-30
precisely because three adversarial seats did that and every gate stayed green.
