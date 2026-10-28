# The reviewed Humble table

`humble-decisions.tsv` is the Humble bundle table after a human decided every
row: 2,199 rows, 2,031 distinct keys, 680 series. `humble-decisions-not-comics.tsv`
holds the five files the reviewer marked as not comics — two light novels and
three audiobooks — so a later `seed` run has a record of them instead of
proposing them again.

It is **not** the shipped dataset. `data/mappings/` is, and a file there is
`include_str!`-embedded in the binary, which is a deliberate act of shipping.
This directory is the reviewed data holding its own copy outside that decision.

## Why it lives here and not in `data/mappings/`

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
second implementation of them:

- `cargo test --workspace` — **606 passed, 0 failed.** That covers
  `MappingRow::validate` on every row (§6.11's forbid-list and 120-byte cap),
  `find_key_collisions` over the joined table including the 12 marketplace rows
  already shipped, and the frozen key pin.
- `cargo xtask check-mappings` — file size within the 2 MiB cap. The file is
  205 KB.

Both changes were reverted. The row cap `check-mappings` enforces is 2,500 per
**pull request**, and this repo's convention is to push to `main`, so that cap
is not what is holding this file here — the licence question is.

## To ship it, once that question is answered

1. `git mv review/humble-decisions.tsv data/mappings/humble.tsv`
2. Add `include_str!("../../../data/mappings/humble.tsv")` to `SHIPPED_FILES`
   in `crates/core/src/mapping.rs` and bump its array size.
3. `cargo test --workspace`, which is what proves step 2 was done.
