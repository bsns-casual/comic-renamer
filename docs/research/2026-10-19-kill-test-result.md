# Kill test result — does corpus lookup beat filename parsing?

**Date:** 2026-10-19
**Question:** squash-normalise the 40 real bundle filenames against a corpus of
published titles. High unique-hit coverage plus a short line→parent map confirms
the reshape; low coverage vindicates the original design.
**Answer: reshape CONFIRMED.**

Throwaway spike. Code lives in the session scratchpad and is not kept.

---

## Corpus used

`heisehis/paperbunkr-gcd-data`, release `gcd-2026-09-15` — a compact SQLite
extract of the Grand Comics Database, published for the Paperbunkr reader.

| | |
|---|---|
| Download | 33.7 MB zip → 97.7 MB SQLite |
| SHA-256 | published in the repo manifest; **verified before use** |
| Series | **229,836** |
| Issues | 2,203,939 |
| Dump date | 2026-09-15 — five weeks old, so recent books in the bundle are present |
| Licence | CC BY-SA 4.0, attribution to the Grand Comics Database™ |
| Schema | `series(id, name, name_key, year_began, year_ended, publisher, language, issue_count)` |

**It already ships the normalisation this design needs.** `name_key` is
lowercase, non-alphanumerics stripped, and leading articles and conjunctions
dropped: `Hellboy, the Corpse and the Iron Shoes` → `hellboycorpseironshoes`.
That is the squash-match hypothesis, pre-built and indexed, by someone solving
the same problem.

---

## Part 1 — coverage

Matching: squash both sides, longest-prefix match against a combined index of
`squash(name)` and `name_key`, then consume volume / format / edition tokens off
the residue. False positives guarded by **boundary cleanliness** — the prefix
boundary must coincide with a separator in the original filename or with a
recognised token start.

| Result | Count |
|---|---|
| Exact title match, no residue | 11 |
| Series + volume cleanly extracted | 20 |
| Series + volume + subtitle leftover | 9 |
| Suspect boundary | **0** |
| **Not found in corpus** | **0** |

**40 of 40 filenames located a real published title.** Including every one of
the five the design document called unreachable without curation:
`mazebook` → *Mazebook*, `anthonybourdainshungryghosts` → *Anthony Bourdain's
Hungry Ghosts*, `xomnibusvolume1` → *X Omnibus* Vol. 1,
`thecompleteelfquest_vol1` → *The Complete ElfQuest* Vol. 1,
`usagiyojimbosagavolume1secondedition` → *Usagi Yojimbo Saga* Vol. 1.

The boundary guard earned its place: a naive longest-prefix match resolved
`witcheromnibus` to the series *Witch* with residue `eromnibus`. Indexing
`squash(name)` and `name_key` together fixed it — *The Witcher Omnibus* is an
exact `name_key` hit, and the combined index lets the longer, cleaner match win.

### The failure mode that matters: ambiguity, not absence

Nothing was missing. Things were **ambiguous** — one normalised key hitting
several distinct titles, usually foreign-language editions of the same book.

**5 of 40 keys were ambiguous, and naive tie-breaking by string length returned
3 silently wrong answers** — a 7.5% confident-wrongness rate, which is precisely
the anti-goal in the design document:

| Filename | Naive pick | Correct |
|---|---|---|
| `maskomnibusvolume1secondedition` | *MASK Omnibus* (Juniorpress, 1987 — the Kenner toy property) | *The Mask Omnibus* (Dark Horse, 2008) |
| `umbrellaacademyvolume1_apocalypsesuite` | *Umbrella Academy* (Delcourt, French) | *The Umbrella Academy* (Dark Horse) |
| `ladysnowblood_vol1` | *Lady SnowBlood* (Dargaud, French) | *Lady Snowblood* (Dark Horse) |

### The fix, and a find worth keeping

Filter candidates by `language = 'en'`, then prefer the publisher **inferred from
the source folder name**. `HumbleDarkHorseAnniversaryBundle` squashes to contain
`darkhorse`, which matches the `publisher` column directly.

| | Naive | Filtered |
|---|---|---|
| Ambiguous keys | 5 / 40 | **1 / 40** |
| Silently wrong answers | 3 | **0** |

All three wrong answers corrected. **Publisher and language disambiguation is a
requirement, not a refinement** — without it the resolver ships a 7.5% silent
error rate on trusted-looking rows.

The folder-name publisher signal is free and was not in any version of the
design. Humble bundles are publisher-themed and say so in the folder name.

---

## Part 2 — sizing the curated line→parent map

The part no database can answer. Measured against the same 40 files.

**20 rows required, covering 20 of the 40 files.** One row per
collected-edition line, not per volume — so the row that maps
`Hellboy Omnibus → Hellboy` covers every future Hellboy omnibus volume at no
additional cost. The other 20 files need no row at all, because their published
title is already the shelf name.

### The automated heuristic failed, which is the finding

A format-word stoplist was run over the 20 lines to see whether the mapping
could be derived rather than curated. It got **at least 4 of 20 wrong**:

| Line | Heuristic produced | Correct | Why it failed |
|---|---|---|---|
| `Usagi Yojimbo Saga` | `Usagi Yojimbo` | `Usagi Yojimbo Saga` | stripped "Saga" — **the exact case the owner flagged as must-not-strip** |
| `The Goon` | `Goon` | `The Goon` | stripped a leading article that is part of the title |
| `The Oddly Pedestrian Life of Christopher Chaos` | `Oddly Pedestrian Life of…` | unchanged | same |
| `The Witcher Omnibus` | `Witcher` | `The Witcher` | stripped both the article and the format word |

The heuristic produced **zero identity rows**. It cannot represent "this format
word is part of the title," which is the only thing that distinguishes
*Usagi Yojimbo Saga* from *Hellboy Omnibus*. This is direct empirical
confirmation that the layer must be curated data, and that the design document's
stoplist (§6.2) would have shipped wrong answers.

---

## Conclusions

1. **Corpus lookup replaces filename parsing.** 40/40 coverage. Word
   segmentation, the stoplist and the title-casing rules are unnecessary.
2. **The corpus is shippable.** 33.7 MB, CC BY-SA 4.0, SHA-256 verified,
   refreshed per GCD dump. It solves first-run coverage with no API key, which no
   live provider can do.
3. **Disambiguation is mandatory.** Language plus publisher, publisher inferred
   from the source folder name. Without it, 7.5% silent error.
4. **The curated map is small and must stay human-authored.** ~20 rows for this
   bundle, keyed per line, stable against new releases — and provably not
   derivable by heuristic.
5. **Default the series folder to the published title.** Then Phase 1 ships with
   zero curated rows, and `--strip-to-parent` becomes opt-in polish backed by the
   map.
6. **Subtitle-after-volume is a real residue shape** — 9 of 40 files. The
   original fixture suite contained one instance and the design treated it as an
   edge case.

## Open, from this test

- 1 of 40 keys stays ambiguous after filtering. Needs a tie-break rule.
- `birdkingvolume1` matched series *Birdking*. Worth a human check against the
  Dark Horse edition before trusting the row.
- Residue parsing should emit a structured subtitle rather than a leftover
  string, since 9 of 40 carry one.

---

# Addendum, 2026-10-20 — held-out validation

The 40-file result above is a **training-set score and it was overfit.** Re-run
against `Humble bundles file list.txt`, supplied by the owner: **2,714 comic
filenames** spanning BPRD, a range of Image titles, and Kodansha manga.

Critically, the list carries no folder boundaries, so **no publisher could be
inferred** — this measures the pessimistic path §6.3 falls through to.

| | Count | Share |
|---|---|---|
| Exact title, nothing left over | 251 | 9.2% |
| Series + volume/format/edition parsed | 1,987 | 73.2% |
| Series parsed, subtitle left unsplit | 267 | 9.8% |
| Boundary rejected as unsafe | 130 | 4.8% |
| No corpus match | 79 | 2.9% |
| **Series located** | **2,505** | **92.3%** |
| of which ambiguous | 128 | 4.7% |

**92.3%, against 100% on the training set.** 92.3% is the figure the design
should quote.

## The result that matters more than the hit rate

**Zero silent false positives across 2,714 files.** Every failure landed in
review. `domesticgirlfriend_vol7` matched the real series *Domes* as a legitimate
prefix and the boundary guard rejected it rather than accepting it. The
confident-wrongness guard held under 68× the data it was designed against.

## Distributors transliterate punctuation into words, inconsistently

The single highest-value finding. Real titles *Hack/Slash* and
*The Wicked + The Divine* arrive as `hackandslash…` and `thewickedandthedivine…`
— and the same catalog also ships `wickedplusthedivine_vol9`, spelling `+` as
"plus" instead. One publisher, one distributor, two encodings of one character.

Fix is corpus-side and needs no word boundaries: index punctuation-expansion
variants of every corpus name — `&`, `+` and `/` each expanded to `and`, plus
`plus` and `slash`. **Worth 3.4 points on its own: 88.9% → 92.3%.**

## Remaining failures, classified

**Mechanically reachable, ~2–3%:** `ogn` and `oneshot` absent from the format
vocabulary; Roman-numeral volumes (`loveandrocketsvoliv_issue3`);
distributor-added publisher prefixes (`ecbloodtype_issue4`); the cross-product of
punctuation and leading-article variants, which `wickedplusthedivine` needs and
the current variant set misses.

**Irreducible without a curated override:** initialisms. `TWD_CompendiumOne`,
`SOA_04`, `GHA_Vol1`. No corpus resolves them and no algorithm guesses them.
**This is the floor under the 7.7%, and it is why a filename→metadata override
layer is required rather than optional.**

**Corpus gaps, concentrated in manga.** *Domestic Girlfriend* is unfindable
across six files, suggesting romanization coverage gaps in the GCD for Kodansha
titles. Worth confirming before Phase 2 promises manga support.

## Methodology correction

An earlier pass surveyed the owner's full comics directory and admitted
`Animal_Man_1-9_2011-2012_Digital-Empire` — a scene release, not a distributor
download — into the evaluation set, then used its issue numbers to argue §1.1's
scope was too narrow. That is circular: scope conclusions drawn from
out-of-scope data. The claim was withdrawn and produced no numbers. **Evaluation
input is distributor bundles only.**
