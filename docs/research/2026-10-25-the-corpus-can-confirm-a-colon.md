# The corpus can confirm a colon

Research for the Humble table review, 2026-10-25. Measured against
`fixtures/humble-ground-truth.tsv` (2,224 verified titles) and the pinned corpus
release `gcd-2026-09-15` (229,836 series).

**The finding in one line: `cargo xtask seed` decomposes 2,224 verified titles
with a string parser, while this project already ships and pins an authority that
can confirm the decomposition.**

## What prompted it

The owner's observation: *"Attack on Titan: Before The Fall is a different series
than Attack on Titan — this is something we can research easily."*

It is, and the corpus answers it outright:

```
  id     name                                year  publisher      issues
 68693   Attack on Titan                     2012  Kodansha USA      34
 81593   Attack on Titan: Before the Fall    2014  Kodansha USA      17
177148   Attack on Titan Omnibus             2021  Kodansha USA      12
 81459   Attack on Titan: Colossal Edition   2014  Kodansha USA       7
126818   Attack on Titan: Junior High        2014  Kodansha USA       5
 81592   Attack on Titan: No Regrets         2014  Kodansha USA       2
126820   Attack on Titan: Lost Girls         2016  Kodansha USA       2
```

Seven distinct series, each with its own id and issue count. So `seed`'s output
for `Attack on Titan: Before the Fall Vol. 9` — series "Attack on Titan",
subtitle "Before the Fall Vol. 9", no volume — is wrong in a way the earlier
write-up mis-stated. The fix is not to rescue the volume marker from the
subtitle: **the series *is* `Attack on Titan: Before the Fall` and the volume is
9**, and 17 issues says volume 9 exists.

## The rule, and how far it goes

> If the title is `<stem> Vol. N` and the corpus has `<stem>` as a series, then
> `<stem>` is the series and `N` is the volume.

Measured over all 2,224 verified titles:

| class | count |
|---|---|
| no trailing volume marker | 1,197 |
| marker, no colon in the stem — the parser already handles these | 900 |
| **marker + a colon in the stem — the problem class** | **127** |
| ↳ corpus confirms the full stem is a series | **111** |
| ↳ corpus has only the pre-colon half | 7 |
| ↳ corpus has neither | 9 |

**111 of 127, or 87%.** The same lookup applied to whole titles resolves 12 more,
where a trailing number is part of the name rather than a volume — `RoboCop 2`,
`B.P.R.D.: 1948`, `Hellboy and the B.P.R.D.: 1955`, `Hellboy Winter Special
2018`. That half is a much thinner seam: only 12 of 405 titles ending in a digit
are whole-title series in the corpus, so it is an exact win rather than a broad
one.

Together, roughly **123 of 2,224 rows (5.5%)** get a decomposition confirmed
against the authority instead of guessed from the string.

## What the 16 unconfirmed cases are

Not noise, and worth knowing before anyone tries to close the gap:

- **Light novels and audiobooks.** `Attack on Titan: Kuklo Unbound (Light Novel)`,
  `Attack on Titan: Harsh Mistress of the City (audiobook)`. The corpus indexes
  comics, so these are correctly absent, and a format suffix in parentheses is a
  separate parsing question.
- **Known corpus gaps.** `Altair: A Record of Battles` — already recorded in
  CLAUDE.md as one of the manga-romanisation absences that §6.2.1's Layer B
  exists for.
- **Near misses on naming.** `The Goon: Bunch of Old Crap Omnibus`,
  `Walt Disney's Mickey Mouse: The Ice Sword Saga`.

## Two traps, both of which cost something to find

**The corpus's `name_key` strips stopwords; a naive key does not.** For series
81593 it is `attackontitanbeforefall` — "the" is dropped. The first run of this
analysis built its own key by lowercasing and keeping alphanumerics, compared it
to `name_key`, and reported `Before the Fall` as **unconfirmed** — contradicting
a direct query made minutes earlier. Comparing a key derived from the `name`
column on both sides raised the confirmed count from 68 to 111. **Anything
matching against this corpus must either reproduce GCD's stopword rules or
compare on `name`.**

*Checked, 2026-10-25: the resolver does not have this asymmetry.*
`Corpus::open` indexes `key_variants(&record.name)` **and** inserts `name_key`,
so both spellings reach the same records and `located` is not depressed by it.
`seed`'s own lookup compares on `name` for the opposite reason: it needs the
string to be the *same* string, not merely stopword-equivalent.

**`issue_count` must not be used to bound a volume.** 107 of the 111 confirmed
stems have `issue_count >= N`, which looks like a free quick check. The four
exceptions are all `Battle Angel Alita: Last Order`, which the corpus records at
15 issues while the bundle holds volumes 16–19 — the corpus is simply incomplete
there. Gating on it would have discarded four correct rows. Use the corpus to
confirm the **series**; never to reject the volume. This is independent support
for D44, which stores `volume_count` per row precisely because deriving a padding
width from a growing table is unsafe.

*And the resolver already does gate on it.* `corpus_ladder`'s step 3 is
"volume feasibility: a hard constraint. A file numbered volume N cannot belong
to a series with fewer than N entries" — the same rule, live, where it costs a
*suggestion* rather than a row. `measure` buckets 93 of 2,223 rows (4.2%) as
"corpus refused (language or volume)", which is the upper bound on what it
costs; the bucket does not yet separate the two causes, and splitting it is the
next cheap measurement if the suggestion rate is ever worth raising.
`MappingRow::validate` carries the rule too, but only for a row with a
`volume_count`, and `seed` writes none — so the four Alita rows are stored, not
rejected.

## What was done with it

**Implemented in `xtask/src/seed.rs` on 2026-10-25** (`trailing_volume`,
`is_series`, `corpus_split`, and a fifth parameter on `build_row`). A run
against the pinned corpus confirms **111 rows**, matching this measurement
exactly, and the generator reports them as their own class rather than folding
them into "clean" — the point is that a reviewer can tell a corpus-confirmed
split from a guess. The colon class fell from 129 to 18, and the known-bad
fraction from 14.7% to 9.7%.

The 12 whole-title confirmations were **deliberately left out**. They change no
field — the parser already keeps a bare trailing number in the series name — so
they would only relabel 12 of 197 flagged rows, which does not change the
review's shape. The lookup is one function (`is_series`) if that ever becomes
worth doing.

Two things that stay true:

- **This does not make the table shippable unreviewed.** D50 and D51 still forbid
  storing rows nobody approved, and "the corpus agrees" is a better guess, not a
  keystroke. It raises the fraction a reviewer can accept at a glance.
- **It introduces no new heuristic.** The corpus is already pinned, already
  shipped, already the resolver's authority, and already carries the licence and
  attribution `data/LICENSE` records. The only change is that the seed generator
  stops ignoring it.

Known-bad fraction before: **14.7%** (8.9% bare trailing number, 5.8% volume
marker swallowed by a colon, 0.0% multiple colons). After: **9.7%**, the colon
class reduced to the 18 titles the corpus cannot speak to — light novels,
audiobooks, `Altair`, and `The Goon: Bunch of Old Crap Omnibus`.
