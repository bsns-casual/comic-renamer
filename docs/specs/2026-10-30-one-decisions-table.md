# One decisions table

**Status:** proposed, 2026-10-30. Supersedes the per-marketplace sharding in
the mapping-first rebuild's §5.2 and §7.3. Decisions D63–D68.

The shipped data becomes one table of decisions, keyed on the filename and
nothing else. A decision is a decision; where it came from is git's problem.

---

## 1. Why

Three separate complaints, one cause.

**The marketplace files are a fiction the code does not believe.**
`shipped_mappings()`'s own doc comment says rows "stay sharded by marketplace on
disk, so `find_key_collisions` must be checked over this joined table, not per
file." The gate already treats it as one table. `Mappings::lookup(key, bundle)`
keys on the *bundle*, not the marketplace, and `scan.rs:110` passes the source
folder's name. Nothing in the logic knows or cares which file a row arrived in.

**`bundle` as part of a row's identity makes 7.7% of decisions unreachable.**
Measured 2026-10-30 over every decision this project holds — the 2,199 reviewed
Humble rows plus the 12 shipped marketplace rows:

```
total rows                  2211
distinct keys               2043
keys with >1 row             158      7.7% of keys
  of those, disagreeing        0
redundant rows removable     168
distinct bundles              41
```

`lookup` is deliberate about multi-row keys: `mapping_store.rs:53` pins
`lookup("alpha", "c").is_none()` with the comment "never silently pick". So a
key carrying two rows resolves **only** for a user whose folder name matches one
of the recorded bundles. The contributor's own folder matches. Nobody else's
does. Those 158 keys are, for every other user, not in the table at all — and
all 158 hold rows that agree on every field, so there was never anything to
disambiguate.

**The Humble lean is in the wrong place.** "Humble" names two unrelated things
and only one of them is a problem:

- a *file* — `data/mappings/humble.tsv`, which does not exist, which is why
  `SHIPPED_MARKETPLACES` and `shipped_mapping_path` had to be invented on
  2026-10-29 so a contribution could refuse to target it;
- a *filename shape* — `resolve.rs:69`'s `is_humble` gates whether the corpus is
  consulted, because Humble ships squashed lowercase stems
  (`popeyevolume1_oliveoylandhersweety`) where Fanatical ships titles
  (`Moonshine Vol. 1`). Different input shapes need different guessing, and D33
  forbids the silent fallback that collapsing them would create.

The first should go. The second is load-bearing and stays.

## 2. What the table becomes

One file, `data/decisions.tsv`, with **one row per key**:

```
key  series  volume  issue  subtitle  lang  volume_count
```

`bundle` is gone **from the shipped table and from the user's local file**. The
key is the whole identity. It stays in the generator's own artifacts — see §2.1,
which is the correction a self-review of this document forced.

**Provenance lives in git.** Every row's origin — who added it, when, in which
commit, with what reasoning in the message — is already recorded, for free, by
the thing that records it better than a column could. A `bundle` column captured
one folder name on one contributor's disk and then degraded every key it touched.

**A correction is a diff.** Under one row per key, `find_key_collisions` stops
meaning "two rows disagree" and starts meaning "a key appears twice", which can
only happen by mistake. Changing a decision edits a line, so a reviewer sees
`- Hacktivist 1` / `+ Hacktivist` instead of a second row appearing somewhere in
a 2,000-line file. That is the whole reason to do this on top of the 158 keys.

### 2.1 `bundle` survives in the generator, and that is not a hedge

The first draft of this document said "drop `bundle`" and would have broken the
review tooling silently. `scripts/review-rows.py:79` keys its decision lookup on
`(key, bundle)` and `sys.exit`s when a row is not found; the review artifact's
stored decisions carry key *and* bundle so a mismatch reports as an orphan
rather than being painted over the wrong file. Dropping the column from
`cargo xtask seed`'s output would orphan all 2,204 stored decisions and remove
the ability to reopen the review, which CLAUDE.md explicitly preserves.

So the split is:

| Artifact | `bundle`? | Why |
|---|---|---|
| `data/decisions.tsv` (shipped) | **no** | identity is the key; §1's 158 keys |
| `<config>/mappings.tsv` (local) | **no** | same table, same rule, same loader |
| a contribution's TSV | **no** | it becomes rows in the shipped table |
| `cargo xtask seed` output + sidecar | **yes** | the generator reads a ground-truth fixture that really does have bundles, and the review page identifies rows by them |
| `fixtures/*-ground-truth.tsv` | **yes** | measurement fixtures naming their real source (D31) |

The generator keeps a column the product does not ship. That is the right shape:
a bundle is a fact about *where a file was downloaded*, which is useful when
building and reviewing a table and meaningless when looking one up.

### 2.2 What dissolves, deliberately

**Invariant 7's bundle half goes away entirely**, and this document is the place
that says so rather than a commit nobody reads. CLAUDE.md marks it hard-won: it
was broken until 2026-10-23, every bundle comparison was literal, and five sites
were routed through `mapping::same_bundle` to fix it — with a specific warning
that folding the gate without folding `commit_session`'s merge would make a
second session on one folder append a row pair the gate refuses, so the user's
own corrections file would stop loading.

All four live call sites exist **because `bundle` is part of a row's identity**:

```
mapping.rs:321   find_key_collisions   key == key && same_bundle(..)
mapping.rs:368   lookup                many.find(|r| same_bundle(..))
map/mod.rs:138   approved_rows         group by key && same_bundle(..)
map/mod.rs:187   commit_session        merge on key && same_bundle(..)
```

Take `bundle` out of identity and there is no bundle comparison left to get
wrong: each of those four becomes `key == key`. `same_bundle` is deleted, not
weakened. **Invariant 7's path half is untouched** — collision comparison over
emitted paths stays NFC-normalised and case-folded, because `Hellboy/` and
`hellboy/` are still one directory on APFS. Only the bundle sentence of the
invariant retires, and it retires because the field it guarded is no longer an
identifier.

This is the one place in this change where the rule "minimise the code, never
the proof" bites. Deleting `same_bundle` deletes its tests. Each of the four
sites must gain a test that a key matches on the key alone, or the dissolution
is indistinguishable from the 2026-10-23 regression it replaced.

### 2.3 A second session on the same folder

`commit_session` merges a session's approved rows into the local file. On one
row per key the rule is **the newer decision replaces the older one, in place**.
It must be a replacement rather than an append, or the local file gains a
duplicate key and stops loading — which is precisely the failure CLAUDE.md's
invariant-7 note warns about, arriving by a different road.

**The motive first written here was wrong, and the rule is still right.** The
draft said "the user mapped that filename again and typed something different".
That cannot happen through `map`: a session is built from *unmapped* files only,
and `scan_once` loads the local layer first, so any key already in the local
file makes its file mapped and keeps it out of the session. What actually
reaches the replacement is a concurrent second session — already marked
`shortcut:` where the read-modify-write has no lock — or a hand-edited file.
Worth knowing before anyone tries to exercise this path from the UI and
concludes the code is dead.

A consequence worth stating plainly: **there is no path through `map` to change
a decision you have already made.** The file is mapped, so it is never offered.

## 3. What does not change

- **Invariant 10, the frozen key rule.** NFC, lowercase, strip non-alphanumerics.
  Keys are not recomputed, not reinterpreted, not migrated. This is a column-set
  change, not a key change, so the 2036-key pin stands and no version bump is
  owed.
- **The suggestion strategy.** `--marketplace` stays a flag and keeps selecting
  how an unmapped file is guessed at. §5 proposes renaming what it describes,
  not removing it.
- **`review/` stays outside `data/`.** The licence gate is "which rows ship",
  which is a different question from "which file rows live in" — and it is
  answered physically, by the directory, with
  `every_tsv_in_data_mappings_is_embedded` failing on arrival to say so. A
  single shipped table does not change that and must not be allowed to: folding
  `review/` into `data/decisions.tsv` would publish the thing the owner
  deliberately deferred on 2026-10-30.
- **Layer B.** `data/overrides.toml` keys on the source folder's name and is
  untouched. It is the one place a folder name is genuinely the right key,
  because it corrects a *guess* rather than recording a decision.
- **Nothing is stored without a keystroke.** Unchanged and still the point.

## 4. Decisions

**D63. One file, `data/decisions.tsv`.** The sharding was cosmetic; the gate
always ran over the joined table. One file also makes the 2 MiB cap mean
something, where per-file caps over a growing set of marketplaces meant nothing.

**D64. One row per key; `bundle` leaves the shipped and local tables.**
Measured: 158 keys carry more than one row and **zero** of them disagree on a
field, so nothing is being discarded — 168 redundant rows go.

The cost is that two genuinely different books whose filenames squash to one key
can no longer both be recorded. **Do not read the measurement as evidence this
will not happen.** All 2,043 keys come from essentially one source — one
person's Humble library plus twelve hand-confirmed marketplace rows — so the
sample says almost nothing about a table fed by many contributors, where
`batman.cbz` from two publishers is an ordinary event.

**Corrected 2026-10-30 after an adversarial review, because the first draft of
this entry oversold itself.** It said the old handling was worse because "both
rows become unreachable for everyone whose folder name does not match, and
nothing reports it". The second half is false. Unreachable meant `lookup`
returned `None`, which made the file **unmapped** — so it appeared in `map`
with a guess and the user typed the title. That is this project's own safe
default, and it is reporting, not silence.

So the honest trade is: D64 swaps "nobody gets an answer and everybody is
asked" for "everybody gets an answer, and for one of them it is wrong and
silent". Three things make it still the right swap, and the third is the one
that matters:

1. All 158 of today's multi-row keys **agree on every field**, so for them the
   new behaviour is strictly better — they resolve for everyone instead of for
   nobody.
2. A collision needs two genuinely different books with one squashed filename,
   both contributed. Never once observed.
3. **The silent-wrong-title hazard is pre-existing and D64 does not create
   it.** A key carrying *one* row has always answered regardless of the folder
   — the old `lookup`'s `[only] => Some(only)` arm ignored the bundle
   entirely. So a wrong row has always produced a confident wrong rename. What
   the two-row state provided was an *accidental* safety net, in a state nobody
   designed for and the contribution gate already refused whenever the fields
   disagreed within a bundle.

What is genuinely not addressed, by this change or anything before it: a mapped
row is never cross-checked against what the resolver would have guessed, and
`map` only ever shows *unmapped* files — so a user handed a wrong title has no
in-tool way to correct it, and nothing prints the path to the local file they
would have to hand-edit. That is a real gap, it predates this change, and it is
recorded in CLAUDE.md's known gaps rather than fixed here.

**D65. Provenance is git history, not a column — in the shipped table.** A
column records one folder name on one disk; git records the author, the date,
the commit and the reasoning. The owner's framing, 2026-10-30: the repo "has no
care for WHERE the data came from so much as it's normalized from all sources".
§2.1 is the limit of this: the generator and the review tooling keep `bundle`,
because they identify rows by it and a self-review caught that dropping it
there would orphan 2,204 stored decisions.

**D66. `SHIPPED_MARKETPLACES` and `shipped_mapping_path` are deleted.** Added
2026-10-29 so a contribution could pick a file and refuse when none shipped.
With one unconditional target the whole branch goes, including the "contributions
are not open for this marketplace" prose. A contribution appends to
`data/decisions.tsv`, always.

**D67. The row cap stays per pull request; the file cap goes to 4 MiB, and the
reason it is not larger is the binary.** Measured: 93 bytes a row, so the
existing 2 MiB cap is about 22,500 decisions — reachable by a table meant to
grow.

The constraint nobody had named is that `data/decisions.tsv` is
`include_str!`-embedded, so **the table lands in the binary byte for byte**. The
release binary is 6.9 MB today. The first draft of this document proposed 8 MiB,
which would more than double the thing Homebrew ships to download a rename tool.
4 MiB is about 45,000 decisions, adds at most 4 MB to the binary, and is a
bound that stays honest about the trade. Past that the table should stop being
embedded, which is a different change and wants its own document.

The 2,500-rows-per-PR cap is unchanged and still PR-only.

**D68. A key already in the table may only be changed, never duplicated.** The
contribution gate rejects a PR that adds a key the table holds; correcting a
decision means editing its line. This is what makes a correction reviewable and
it is the half of D64 that does the work.

## 5. Open, with recommendations

**O-1. Rename `--marketplace`.** What it actually selects is filename shape:
squashed stems that need the corpus, versus names that are already titles. A
truer flag is `--filenames squashed|titles`, with `--marketplace` kept as a
hidden alias so no documented command breaks. **Recommend doing it**, in a
separate change after the table lands, because it touches `resolve.rs`,
`seed`, `measure` and the README, and bundling it would make the table's diff
unreviewable.

**O-2. What `seed` writes.** It currently emits `target/seed/humble.tsv` with a
`bundle` column, and `scripts/review-rows.py` reads exactly those eight fields
in order. **Recommend leaving `seed`'s output format alone entirely** — not even
the filename — until the review is known to be closed for good. The shipped
table changing shape does not require the generator's intermediate to change
shape, and §2.1 is why. Renaming is cosmetic and costs a working review page.

**O-3. Whether `lang` stays a column.** A key is a filename and a filename is
one file in one language, so `lang` is a field and not identity. It is read by
the resolver's language gate. **Recommend keeping it**, unexamined, because
nothing in this change touches it.

## 6. What this is not

It is not a step toward publishing the Humble table, and it must not become one
by accident. The licence question is untouched and still open: `data/LICENSE`
binds rule data to the Grand Comics Database under CC BY-SA 4.0, 239 of 2,223
reviewed rows carry a field the corpus decided, and whether a 2,199-row table
derived from one person's Humble order history may be redistributed at all is a
terms-of-service question nobody here has answered. The repository is private.
`review/humble-decisions.tsv` stays in `review/`.
