# Comic Renamer — Design

**Status:** draft, pending owner approval
**Date:** 2026-10-19
**Owners:** Mike (engineering), Rob (UX)
**Org:** Business Casual

Supersedes `2026-10-19-comic-renamer-design-v1-superseded.md`, whose resolver
architecture was wrong. Why, and the evidence that replaced it:
`docs/research/2026-10-19-adversarial-review-findings.md` and
`docs/research/2026-10-19-kill-test-result.md`. The v1 document is retained only
so those section references resolve.

---

## 1. Intent

People who buy comics in bulk receive files whose names are hostile to every
comic library application in existence. `hellboyomnibusvolume1_seedofdestruction.pdf`
is a real filename from a real bundle. Dropped into Komga, it produces a series
called "hellboyomnibusvolume1 seedofdestruction" with no volume number.

Comic Renamer points at a folder of those files, identifies what each one
actually is, shows the user every proposed rename, and only after approval puts
correctly-named files where their chosen library app expects them.

It is not a reader, not a library manager, not a metadata tagger.

### 1.1 The real problem shape

Measured against a real bundle — `HumbleDarkHorseAnniversaryBundle`, 40 files,
**31.2 GiB**, largest single file 2.54 GiB:

1. **No issue numbers.** Every file is a collected edition — omnibuses, `vol1`,
   archives, one-shots. Nothing maps to `{series} - #{issue}`.
2. **No word boundaries.** `plantsvszombieszomnibusvolume3`, `xomnibusvolume1`,
   `mazebook`. Separators, where present, are inconsistent: the underscore in
   `hellboyomnibusvolume1_seedofdestruction` marks a subtitle, while those in
   `nemesis_rogues_gallery` are plain spaces.
3. **Recovering the title is a knowledge problem.** `mazebook` is correctly one
   word; `xomnibus` is two. No segmentation algorithm distinguishes them,
   because the information is not in the string.
4. **File contents pay nothing.** PDF has no `ComicInfo.xml` equivalent, and the
   one file inspected carried an empty title with an author field reading
   `Adobe InDesign CC 2014 (Macintosh)` — the layout tool, not the book. Parsing
   31 GiB to recover nothing is a bad trade.

### 1.2 The conclusion v1 got wrong

v1 reasoned: recovering the title needs world knowledge, therefore we must
hand-curate that knowledge, therefore the curated list is the product. The
missing step is that **this knowledge is already published and downloadable.**
You do not reconstruct the apostrophe in `Bourdain's` — you look up a record
that already contains it.

**Measured, not assumed.** Squash-normalising both the filename and a corpus of
published titles, then longest-prefix matching, located **40 of 40** filenames in
the reference bundle, including all five v1 called unreachable — and **2,505 of
2,714 (92.3%)** across a held-out set of real distributor filenames spanning
three publishers, with no publisher hint available.

The 40/40 is a training-set score. **92.3% is the number to plan against.**

**Word segmentation and the title-casing rules are unnecessary *for series
titles*,** which is where they failed — `mazebook` versus `xomnibus`. They are
not unnecessary in general: subtitle recovery is an unsolved sub-problem with no
corpus answer, and §6.4 states honestly what Phase 1 does instead.

**What a corpus cannot do**, verified against the `gcd-django` models and the
Metron and Comic Vine schemas: tell you which shelf a collected edition belongs
on. `gcd_series` has no parent field, no "collects" field, and a three-value
publication-type vocabulary that cannot express *that something is a
collection*. That judgment is the only thing we curate.

> A corpus can tell you the title is *The Complete ElfQuest*.
> Nothing can tell you it belongs in `ElfQuest/`.

---

## 2. Goals

- Turn a folder of badly-named comic files into correctly-named files laid out
  the way the user's chosen app expects.
- Never modify or move anything without explicit confirmation of a reviewed plan.
- Be honest per file about where the answer came from and how much to trust it.
- Support several target applications and make adding the next one cheap.
- Work on a first run, for a stranger, with no account and no API key.

## 3. Non-goals

Reading comics. Browsing or managing a library. Converting file formats
(PDF→CBZ is backlogged — see §8). Opening, rasterising or inspecting the inside
of any file. Background folder watching. Accounts, cloud sync, paid tiers.
Guaranteeing the destination app can read what it renames.

## 4. Anti-goals

- **Silent data loss.** Unrecoverable reputational damage for a file-mutating
  tool from an unknown publisher.
- **Confident wrongness.** A plausible wrong name approved without scrutiny is
  worse than an obvious failure. This is not hypothetical: naive tie-breaking in
  the kill test produced a **7.5% silently-wrong rate** on rows that would have
  been marked `ready`, including resolving *The Mask Omnibus* to the Kenner toy
  property *MASK*. §6.3 exists because of that measurement.
- **A corpus nobody can correct.** If fixing a wrong row requires understanding
  code, outside contributions stop.
- **Requiring the user to manage a runtime.** If someone has to install a
  language runtime, match versions, or hand-edit a PATH, the intended audience
  never gets past step one. A package manager that does this *for* them is
  acceptable — Homebrew needs Command Line Tools and its own prefix on `PATH`,
  which is a one-time cost it pays itself (§6.12).

---

## 5. Constraints

| Constraint | Source | Consequence |
|---|---|---|
| First release is a terminal program | Owner | Genuinely polished, not a bare script. Assume low technical skill. |
| Command-line tool only | Owner | A desktop GUI is **not** in the plan and is not assumed. The core is still a library crate with no CLI assumptions, because that is what makes it testable — not because a GUI is coming. |
| Install is one command | Audience | Homebrew for release one (§6.12). |
| Filename-only, never open files | §1.1 | No PDF or archive parsing anywhere. |
| First run must work with no credentials | §9 data terms | The corpus is available offline on a first run. No live API can be load-bearing. Delivery mechanism is open — see 11.3. |
| Contributions arrive as GitHub PRs | Owner | Rule data is plain, diffable, and **never path-shaped** (§6.11). |
| Code MIT, corpus CC BY-SA 4.0 | D3, D9 | Separate directories, separate licence files, GCD attribution. |

---

## 6. Architecture

Three stages. Each has one job and hands structured data to the next.

```
filename ─▶ ① TITLE RESOLUTION ─▶ ② SHELF DERIVATION ─▶ ③ PRESET ─▶ destination
              corpus + disambig.      published title       Komga/Kavita/…
              │                       or curated map        (sole path
              └─ live API (BYOK)      (opt-in)               constructor)
```

### 6.1 Stage ① — title resolution

Input: a filename. Output: `{ series, volume?, format?, edition?, subtitle? }`.

**The bundled corpus is the primary source**, not a fallback.
`heisehis/paperbunkr-gcd-data` — a 33.7 MB SQLite extract of the Grand Comics
Database, 229,836 series, CC BY-SA 4.0, SHA-256 published per release. It
already ships `name_key`: lowercase, non-alphanumerics stripped, leading
articles and conjunctions dropped. That is this design's normalisation,
pre-built and indexed.

Matching: squash the filename, longest-prefix match against a **combined index**
of `squash(name)`, `name_key`, and punctuation-expansion variants, then consume
volume, format and edition tokens off the residue.

**Punctuation-expansion variants are required, not an optimisation.**
Distributors transliterate punctuation into words, and do it inconsistently
within one catalog: *Hack/Slash* ships as `hackandslash…`, and
*The Wicked + The Divine* ships as both `thewickedandthedivine…` **and**
`wickedplusthedivine…`. So every corpus name is additionally indexed with `&`,
`+` and `/` expanded to `and`, and to `plus` and `slash`. Measured worth: **3.4
points, 88.9% → 92.3%.** The fix is corpus-side and therefore needs no word
boundaries in the filename, which is why it works at all.

Inputs are also pre-cleaned of distributor artefacts — trailing numeric ids like
`_1420484117`, `_optimized` markers — and non-comic files (`id_*`, readme, cover
art) are filtered out before matching rather than reported as failures.

**Both keys must be indexed together.** Indexing them separately resolved
`witcheromnibus` to the series *Witch* with residue `eromnibus`; combined, *The
Witcher Omnibus* wins as an exact `name_key` hit.

**False positives are guarded by boundary cleanliness.** A prefix match counts
only if the boundary coincides with a separator in the original filename or with
a recognised token start. This guard produced zero false positives across the
reference bundle.

If the corpus misses, stage ① may consult a live API (§6.6). If that misses too,
the row is `failed`.

### 6.2 Stage ② — shelf derivation

**Default: the series folder is the published title.** Always available, never
needs curated data, never wrong — the folder *is* what the book is called.

**`--strip-to-parent` is opt-in**, backed by a curated map keyed per
collected-edition **line**, not per volume:

```
Hellboy Omnibus        -> Hellboy
The Complete ElfQuest  -> ElfQuest
X Omnibus              -> X
Usagi Yojimbo Saga     -> Usagi Yojimbo Saga      # identity: do not strip
```

One row covers every volume in that line, now and in future, so the map is
small and stable. The reference bundle needs **20 rows for 40 files**; the other
20 files need none.

**This map cannot be derived.** A format-word stoplist was measured against
those 20 lines and got at least 4 wrong, including `Usagi Yojimbo Saga →
Usagi Yojimbo` and `The Goon → Goon`. It produced zero identity rows, because a
stoplist structurally cannot express "this format word is part of the title."
v1's §6.2 stoplist would have shipped wrong answers.

### 6.2.1 The two curated layers

v1 conflated these, and v2's first draft named one while assuming the other.
They are separate files with separate schemas and separate review burdens.

**Layer A — the line→parent map.** Two columns, keyed per collected-edition line,
used only by `--strip-to-parent`. Described above. ~20 rows for the reference
bundle.

**Layer B — the filename override.** Maps a filename pattern to resolved
metadata, for what the corpus provably cannot reach. The held-out run puts a hard
floor under this: **initialisms.** `TWD_CompendiumOne`, `SOA_04`, `GHA_Vol1` — no
corpus resolves them and no algorithm guesses them. Corpus gaps sit here too,
concentrated in manga romanisation.

Layer B is **not optional polish** — it is the floor under the residual 7.7%, and
it is the only place a *wrong corpus match* can be corrected. Without it,
anti-goal §4's "a corpus nobody can correct" is unavoidable, because Layer A can
only fix shelves, never titles.

**Layer B cannot key on a bare filename.** `vol1.pdf`, `001.pdf` and `cover.pdf`
recur across unrelated publishers, so a bare-filename key would match the wrong
book. It needs a discriminator — a sibling-set fingerprint, a containing folder
name, or a required publisher field. Choosing one is Phase 3 design work; the
constraint is recorded here so it is not discovered late.

### 6.3 Disambiguation — mandatory, not a refinement

One normalised key frequently hits several distinct titles, usually
foreign-language editions. **5 of 40 keys in the reference bundle**, and naive
tie-breaking by string length returned **3 silently wrong answers.**

**Ambiguity is measured over corpus *records*, never over title strings.** A
record is (name, publisher, year, run length). Three different books all called
*Velvet* — Malibu 1993, Image 2013, Image 2014 — are one title and three records,
and comparing titles reports them as unambiguous. That is the single most
dangerous case, so comparing titles is blind precisely where it matters. Measured
cost of getting this wrong: it inflates the apparent `ready` rate from 40% to
79%, every inflated row being a confident guess.

Resolution, in order:

1. Filter to the user's language (default `en`). **Hard filter — and when no
   record matches the language, the row goes to `review`, it does not fall back
   to any language.** Measured: a silent fallback caused 6 of the 7 total errors
   across 2,223 graded rows, resolving `noragami_vol1`–`vol6` to the Japanese
   original `ノラガミ [Noragami]` from 講談社 instead of the Kodansha USA edition —
   which would have produced a Japanese-script folder name.
2. Prefer the publisher **inferred from the source folder name**.
   `HumbleDarkHorseAnniversaryBundle` squashes to contain `darkhorse`, matching
   the corpus `publisher` column. Free signal, but **worth only ~7 points at
   scale** — on 2,714 held-out files it resolves 7.4% and leaves 28.9% ambiguous
   *within a single publisher*. An earlier draft overstated it from a 40-file
   sample.
3. **Volume feasibility. A hard constraint, not a preference:** a file numbered
   volume N cannot belong to a series with fewer than N entries. Resolves 1.7%
   outright and narrows the rest.
4. **Shortest feasible run** — among candidates with enough volumes, prefer the
   shortest, on the reasoning that a short series is a collected-edition line
   while a long one is the monthly run. `Velvet` Image-2014 with 3 entries over
   Image-2013 with 15. **This is a heuristic and it carries 39 of the 71
   percentage points of `ready`.**
5. Still ambiguous → `review`, listing every candidate with publisher, year and
   run length. **Never silently pick.**

**Step 4 is validated and may promote to `ready`: 0 errors in 658 graded rows.**
It was held on probation until measured, because it decides more rows than
everything else combined on an unproven prior. The measurement (§7) cleared it
outright — it is the *lowest*-error basis of the three, which is the opposite of
what was feared. Re-validate it on any corpus bump.

"Prefer the earliest edition," which an earlier draft listed, is **withdrawn**:
on a title shared by unrelated books it systematically picks the oldest obscure
one over the book the user actually bought — it resolved `Velvet_vol1` to Malibu
1993 inside an Image bundle.

**Step 1 is a hard filter; step 2 is a preference; step 3 is a weak preference
that may not promote a row to `ready` on its own.** The distinction matters
because step 3 is an unvalidated heuristic of exactly the kind §4 forbids: a row
whose ambiguity is broken *only* by "earliest edition" stays `review`.

**The no-publisher case is the common case, not the exception.** A user running
against `~/Downloads/comics` yields no publisher signal at all. Measured on a
held-out set of 2,714 filenames with no inferable publisher: **4.7% ambiguous.**
With a publisher inferable, the reference bundle went from 5/40 ambiguous and 3
silently wrong to **1/40 ambiguous and 0 wrong**. So the folder-name signal is
worth a great deal when present and must never be assumed present.

### 6.4 Residue parsing, and what Phase 1 does not do

After the series prefix is consumed, the residue carries volume, format, edition
and subtitle. The observed vocabulary is wider than v1 assumed and was derived
from real data, not invented: numeric and word volumes (`volume1`, `vol1`,
`bookone`), Roman numerals (`voliv`), and format tokens `tp`, `tpb`, `hc`, `sc`,
`gn`, `ogn`, `oneshot`, `deluxe`, `trades` alongside `omnibus` and `archives`.
Edition qualifiers (`secondedition`) are parsed and dropped from output.

**Subtitles are not recovered in Phase 1, and the design should not pretend
otherwise.** A subtitle arrives squashed — `seedofdestruction` — and there is no
source for the split:

- The corpus `series` table has no subtitle field.
- Looking the residue up as its own series name **does not work by
  prefix-matching.** GCD names that book `Hellboy: Seed of Destruction`, keyed
  `hellboyseedofdestruction`, so `seedofdestruction` is a *suffix*. Reaching it
  needs substring search over 229,836 series, which discards the
  boundary-cleanliness guard — and discarding that guard is precisely what
  produced `witcheromnibus` → *Witch*. Not worth trading a correctness guard for
  a cosmetic field.
- Dictionary segmentation would work on most subtitles, since these are ordinary
  English words rather than the title-identity problem that defeated it. But it
  loses apostrophes (`theassassinsroad` → *The Assassin's Road*), so its output
  cannot be trusted unreviewed.

**Phase 1 therefore carries the subtitle as the unsplit residue and marks the row
`review`.** Measured cost: 9.8% of the held-out set. Recovery is a later phase
with the three options above costed; it is not a Phase 1 requirement and no
fixture asserts a split subtitle.

### 6.5 Corpus distribution and refresh

**The corpus binary is never committed.** The repository holds a small manifest —
dump date, release URL, SHA-256, byte size, schema version — mirroring what the
upstream extract already publishes. The tool fetches and verifies against that
hash on first use, then caches locally. Committing a 33.7 MB SQLite file monthly
would add roughly 400 MB of permanent git history per year for a file that is
regenerated upstream anyway.

**Refresh is automated and gated by a human.** A monthly scheduled GitHub Actions
workflow checks upstream for a newer dump and, when it finds one, opens a
**pull request that bumps the manifest only**. A maintainer merges it. The
workflow never pushes to the default branch, because a corpus bump changes
resolution behaviour for every user.

**The coverage suite is the gate.** Every manifest-bump PR runs the full held-out
measurement (§7) and the PR body carries the delta. A refresh that drops located
coverage — or far worse, introduces a silent false positive — is a blocked merge,
not a routine update.

Corpus version is recorded in every plan, so a resolution can be reproduced and a
bad row traced to a specific release.

### 6.6 Stage ① fallback — live lookup, bring-your-own-key

Only for corpus misses, only with a key the user supplies, never bundled, never
load-bearing. Rows resolved this way are `review`, never `ready`. See §9 for why
no provider can be the coverage story.

### 6.7 Status model

| Status | Meaning |
|---|---|
| `ready` | Corpus match, unambiguous after §6.3, residue fully consumed. Executable without edit. |
| `review` | Ambiguous candidates, an unparsed subtitle, or a live-API match. Requires a human decision. |
| `failed` | No match. Skipped unless the user supplies a title. |

Every row records which stage and source produced it, the corpus version, and
the matched record's id so it can be reported as wrong.

### 6.8 Presets

A preset owns folder structure and filename format for one target app, and is
**the only component that constructs paths**. Launch set: Komga, Kavita, Panels
(see §8 phase 2 and open question 11.2).

Presets also **sanitise**. The corpus returns real titles containing characters
that are illegal or dangerous in filenames — `Nemesis: Rogues' Gallery` has both
a colon, which is forbidden on Windows and renders as `/` in Finder, and an
apostrophe. Sanitising is first-party code's job, not the data's.

### 6.8.1 The Komga preset, specified

Researched against the source, not the docs, because the behaviour that matters
is undocumented.

**Komga has no filename parser.** Its metadata providers are ComicInfo.xml
(CBZ/CBR only), EPUB OPF, Mylar `series.json`, ISBN barcode, local artwork and
one-shot. There is no PDF provider and no filename provider. **For a PDF corpus
our output is the entire metadata record** — nothing else is ever read.

| Komga behaviour | Consequence for the preset |
|---|---|
| `Series.name` = **leaf folder name only**; ancestors never read | Emit flat `<Series>/<Book>.<ext>`. Nesting is organisational for humans and invisible to Komga. |
| `Book.name` = filename without extension; `BookMetadata.title` = that | The filename *is* the displayed title. |
| `number`/`numberSort` assigned **positionally** after a natural sort | Filename sort order *is* reading order. Volume must precede subtitle. |
| Natural sort compares digit runs numerically | Komga does not need zero-padding — `Vol. 2` precedes `Vol. 10` unpadded. **We pad anyway** (D37), because every other tool sorts lexically. |
| A volume-numbered file with no issue number is an ordinary book | **No "Special" bucket.** Our entire corpus shelves normally. This is why Komga is the launch target. |
| Colon, apostrophe, `#` | No parsing or display effect; they land verbatim in the title. Filesystem and transport only — `:` is legal on Linux but breaks over SMB, `#` is harmlessly percent-encoded in the stored URL. |
| One-shots are **folder**-triggered | Library option `oneshotsDirectory`, case-insensitive substring match on the full path. Never emit into such a path unintentionally. |
| Collections and read lists | From ComicInfo `SeriesGroup` / `AlternateSeries`, both **CBZ/CBR only** — so for PDFs they are UI/API only and out of scope. |
| PDF is rendered server-side to page images | No embedded metadata read. Confirms the filename-only constraint from the other direction. |
| Unreadable file | Media status *Error* or *Unsupported*; the book still appears but will not open. |

Preset effort: **small.** The real work is the pre-flight below.

### 6.8.2 Komga pre-flight — required, not advisory

**A rename is a delete-plus-add, not an update.** Komga keys books by exact file
path. It rematches renamed files by **file size, then file hash**, and on a match
restores media analysis, read progress, read lists, collections and user
thumbnails.

**Hashing must already be on before the rename. Enabling it afterwards cannot
help.** `tryRestoreBooks` only considers deleted records that already carry a
hash, and `tryRestoreSeries` requires every deleted book to carry one. Switching
hashing on hashes books by reading their *current* path — which after a rename no
longer exists, so a deleted record can never acquire a hash retroactively. A
second trap: `emptyTrashAfterScan` purges deleted records immediately, making
restore impossible whatever the hash setting.

So, before renaming into an existing Komga library:

```
GET /api/v1/libraries
  assert hashFiles === true
  assert emptyTrashAfterScan === false
  if hashing was only just enabled, let the hash tasks drain first
```

`hashFiles` defaults to **true** and `emptyTrashAfterScan` to **false**, so the
common case passes — but a user who changed either loses read progress silently,
and silence is the problem. Refuse rather than warn.

One narrower note: an **unlocked** title follows a rename, while a title the user
hand-edited in Komga's UI is locked and will not update. Only hand-curated
libraries are affected.

Naming rules, as decided by the owner: volume always abbreviated `Vol. NN`,
**zero-padded** (D37); subtitle separator ` - `; real-title oddities preserved,
so `Zomnibus` stays `Zomnibus`. Casing comes from the corpus, which also corrects
by hand: it returns `The Complete ElfQuest` and `Nemesis: Rogues' Gallery`, both
more accurate than the hand-written fixtures.

**Padding width is per series, taken from the corpus.** All files in a folder
belong to one series, so the width is the digit count of that series' highest
volume — `issue_count`, which the resolver already reads — with a floor of two.
A nine-volume series gets `Vol. 01`–`Vol. 09`; a 34-volume manga run gets
`Vol. 01`–`Vol. 34`; a 120-volume run gets `Vol. 001`. A fixed two-digit width
would silently break lexical order at volume 100, and manga runs reach it.

### 6.9 The plan

`scan` produces a plan and never touches the destination. Each row carries a
stable row id, source path, **resolved metadata**, status, provenance, a reason
when not `ready`, and the destination path.

**`apply` recomputes the destination from metadata and the preset. It never
trusts the stored path.** The stored path is display data so a reviewer can see
the outcome; it is not an instruction. Otherwise §6.11's threat model is
reintroduced one layer up, by a file this design explicitly hands to AI
assistants and attaches to public pull requests.

Consequence for both review surfaces: **"edit" means editing metadata fields,
not paths.** A user cannot express an arbitrary destination, which is the point —
the preset remains the sole path constructor. A row whose recomputed path differs
from the stored one is a tampering signal and refuses.

**Between one and two rows in three will need a human — not one in five.**
Measured over 2,714 held-out filenames with ambiguity counted over records:

```
ready, unambiguous outright          30.0%
ready via volume feasibility          1.7%
ready via shortest-run heuristic     39.3%   <- unvalidated, see 6.3
still ambiguous                      11.6%
subtitle unsplit                      9.8%
not located                           7.5%
```

So `ready` is **71.1%** if the heuristic is trusted and **31.7%** if it is not.
An earlier draft put it at 78% by counting ambiguity over titles. The plan
file is therefore the **normal** path, not an escape hatch for unusually large
runs, and open question 11.5 — whether `apply` refuses rows still marked
`review` — is Phase-1 blocking rather than deferrable.

Two review surfaces, **selected by total row count, not review count** — keying
on review count means a 500-row run with 20 reviews auto-applies 480 unseen
rows:

- **Small runs:** an interactive terminal walk — accept, skip, edit.
- **Large runs:** the plan is written to a file, edited by the user or an AI
  assistant, then applied. Bulk editing beats any terminal UI at several hundred
  rows, and the file is diffable and resumable.

Both surfaces share **one contract**: every state the walk can produce has a
file representation, including skip. The plan carries a checksum so `apply` can
tell what a human changed.

**Not TSV.** Tabs occur in titles, Excel corrupts encoding and line endings, and
leading `=`/`+`/`-`/`@` in a title is formula injection from an
attacker-controlled filename. The format must carry row identity and survive a
round trip through an editor.

**Plans must not leak.** They are handed to AI assistants and attached to public
PRs. No absolute paths outside the chosen roots, and no credentials — including
in error messages, where an unredacted API URL would carry the user's key.

### 6.10 Execution and safety

**Moves by default**, with `--copy` and `--hardlink`. The reference bundle is
31.2 GiB and the development machine has 11.0 GiB free, so copy-by-default
cannot complete on the project's own corpus.

| Mode | Extra disk | Originals | Constraint |
|---|---|---|---|
| `--move` (default) | none, same volume | gone from source | cross-volume degrades to copy + verified delete |
| `--copy` | none when cloned, else equal to payload | kept | prefers `clonefile(2)`; falls back to streaming off-APFS or cross-volume |
| `--hardlink` | none | kept | same volume only; cloud-sync clients mishandle links |

A same-volume move is `rename()` — atomic, zero space, and it eliminates the
truncated-multi-gigabyte-PDF failure entirely.

**APFS cloning becomes available with D17 and is the preferred copy mechanism.**
This is a direct consequence of the language switch. The earlier draft recorded
that cloning was unreachable, which was true of Node — `COPYFILE_FICLONE`
silently degrades to a full copy and `COPYFILE_FICLONE_FORCE` returns `ENOSYS` on
same-volume APFS, so Node can neither force nor verify a clone. Rust can call
`clonefile(2)` directly and check its return.

A clone is copy-on-write: **originals kept, zero extra disk, no shared-inode
surprises.** Strictly better than a hardlink for copy semantics, and strictly
better than streaming for cost.

**Verified on this project's own hardware**, not assumed: a 200 MB clone moved
`df` available by 92 KB, produced a distinct inode rather than a shared one, and
**returned `EEXIST` when the destination already existed** — so unlike `rename()`
it is natively no-clobber. It also works on zero-byte and sparse sources.

Three constraints. It is APFS and same-volume only, so it must be **probed at
runtime with a fallback to streaming**, never assumed.

**And cloning a cloud placeholder silently hydrates it.** Measured against a real
Dropbox placeholder: `clonefile` succeeded, but the source went from
`blocks=0, SF_DATALESS` to `blocks=384, flags cleared` — the file was downloaded
to satisfy the clone. A clone from a **materialised** source in the same tree
moved `df` by 16 KB for a 6 MB file, essentially free.

So cloning is free **only when the source is already local.** Against a library
of online-only files it triggers a full download first — for the reference
bundle, 31.2 GiB of it — and the clone is free only afterwards. This makes
`SF_DATALESS` detection at `scan` load-bearing rather than informational. And `du` and `stat -f %b` cannot see clone
savings; only a `df` delta reveals the truth, so capacity reporting must not use
them (§6.10 `scan`).

**On a cloud-sync source, a move is a server-side delete.** Refuse `--move` when
the source root is a cloud-sync folder unless explicitly overridden.

**Mode is disclosed before execution, with real numbers** — not a flag in
`--help`:

```
  MOVE MODE — files will be relocated, not copied.

  412 files · 31.2 GiB · same volume → instant, no extra disk used
  Originals will no longer exist at their current paths.

  38 files are cloud placeholders — 6.1 GiB will be DOWNLOADED first.

  --copy       keeps originals — clone available here, 0 GiB extra disk
  --hardlink   keeps originals, no extra disk, same volume only

  Type MOVE to continue:
```

**The disclosure reports download cost, not only disk cost.** Those are
different numbers and a user with an online-only library cares more about the
first. A plan whose sources are dataless will transfer gigabytes whatever
mechanism is chosen, and saying "0 GiB extra disk" while silently pulling 6 GiB
over the network would be exactly the kind of quiet surprise §4 forbids.

Typed confirmation on first run and whenever the plan is cross-volume.

**Size zero is never an error.** The tool must not treat an empty source file as
missing, corrupt or suspicious. Three distinct size checks exist and all three are
correct on a zero-byte file: the free-space preflight sums to zero and passes;
post-copy verification compares `0 == 0` with matching empty hashes; and
and placeholder detection reads `SF_DATALESS`, which a zero-byte file does not
carry.

**`st_blocks == 0 && st_size > 0` is the wrong predicate and must not be used.**
Verified: a `truncate`-created 5 GiB sparse file reports `st_size=5368709120`,
`st_blocks=0`, `st_flags=0`. That predicate misclassifies every legitimate sparse
file as a cloud placeholder — including this project's own streaming fixture
(§7). Stated explicitly because the fixture suite (§7) is built from
generated zero-byte files, and an "empty means broken" guard added in good faith
would silently disable the whole suite.

**`scan`, in order:** `realpath` both roots and **refuse if either contains the
other** — otherwise a recursive scan re-ingests its own output; `stat().dev` for
same-volume; probe and record destination case- and normalisation-sensitivity;
sum **logical** sizes via `stat().size`, never `du`, which reports neither
cloud-placeholder nor clone reality; flag dataless sources by the **`SF_DATALESS`
flag in `st_flags`** (`0x40000000`); record per-row mechanism and required space.

**`apply`, before touching anything:** re-stat every source against the plan and
refuse changed rows — the reference bundle's file count read 42 then 40 during
review, so the source set genuinely mutates; re-check space for streaming rows,
refusing the whole run when required exceeds available minus `max(1 GiB, 10%)`;
re-verify the mechanism and **never silently fall back to streaming 31 GiB**.
Per row, **by mechanism** — the pipeline is not uniform:

- **`rename` (same-volume move):** atomic. No temp file, no copy, nothing to
  hash. Verification is that the call succeeded.
- **`hardlink`:** `link()` then confirm both paths share an inode.
- **`stream` (copy, or cross-volume move):** temp → `fsync` → verify size and
  hash → `rename` into place → and only then, for a move, unlink the source.

Journaled for `--resume` and `undo`. Destinations opened `O_EXCL`.

**Every run writes an undo manifest, and writes it as it goes.** Original path,
final path, mechanism and status — one row per operation, appended and flushed
*before* each operation is attempted and marked on completion. Written after the
fact, an interrupted run would leave files moved with no record of where they came
from, which is the one state no recovery can reason about.

```
"original_path","new_path","mechanism","status"
"bprd_1948.pdf","B.P.R.D/B.P.R.D 1948.pdf","rename","done"
```

`comic-renamer undo <manifest>` reverses it in reverse order. The manifest is
also an audit trail a user can read in any editor without the tool.

**Quoting is not optional.** Comic titles contain commas — the GCD lists
*Hellboy, the Corpse and the Iron Shoes* — so rows are RFC 4180 quoted with
embedded quotes doubled. A leading `=`, `+`, `-` or `@` is prefixed to defuse
spreadsheet formula injection, since these strings originate in
attacker-controlled filenames (§6.11).

**Collision detection compares on a normalised key, never a path string.** On
APFS, `Hellboy/` and `hellboy/` fold into one directory and the second write
silently overwrites the first; NFC and NFD spellings of one title do the same,
and `readdir` then returns the NFC form so the tool cannot observe what it did.
Under `--move` that destroys the only copy. The filesystem half of this is demonstrated and stands. **The Komga half does
not, and an earlier draft overstated it.** Komga sorts and matches through ICU
collators configured with `CANONICAL_DECOMPOSITION`, registered as SQLite
collations, so NFC and NFD compare *equal* — the two forms do not produce two
series. APFS is normalisation-preserving and NFC from macOS survives the copy
intact. What remains true is narrower: the stored path URL is matched by exact
string equality, so changing an existing file's normalisation reads as
delete-plus-add, which is the §6.8.2 restore problem rather than a duplication
problem.

**Detection alone is not sufficient — the move path needs an atomic no-clobber
primitive.** A same-volume move is `rename(2)`, which **silently overwrites** an
existing destination. "Destinations opened `O_EXCL`" covers only the streaming
path. So a collision check followed by a `rename()` leaves a TOCTOU window in
which, under move-by-default, the only copy of a file is destroyed — exactly D2's
failure, reached through the mechanism D1 chose.

| Mechanism | No-clobber primitive |
|---|---|
| move, same volume | **`renamex_np(src, dst, RENAME_EXCL)`** — fails rather than overwrites |
| clone | `clonefile(2)` returns `EEXIST` natively — **verified** |
| stream | `O_EXCL` on the destination |
| hardlink | `link(2)` fails if the destination exists |

Every mechanism must refuse **at the syscall**, not merely be checked beforehand.
The pre-flight check exists to give the user a reviewable plan; the syscall flag
exists because that check cannot be trusted at the moment of the write.

Requirements: compare NFC-normalised and case-folded; **emit NFC always, as a
hard invariant**; normalise `'` versus `U+2019`; and treat an existing file at
the destination as a first-class collision — merging into a library that already
contains `Hellboy/` is the common case, not the rare one. Policy: **refuse the
row.** Never overwrite, never auto-suffix.

### 6.11 Rule-data safety

v1 claimed rule data is "safe by construction" because it is data and not code.
**That is false: a destination path string is an arbitrary-write primitive.**
`path.join(dest, "../../..")` escapes, and a `startsWith(dest + sep)` check
passes while a symlinked subdirectory writes outside the tree. The attack that
defeats human review: `C:\...` and `\\server\share` are inert strings on a macOS
reviewer's machine and absolute paths on a contributor's Windows box.

**Rules carry metadata fields only** — series, volume, format class, edition
class, subtitle — from a closed vocabulary. Rules never contain paths or
separators. The preset is the sole path constructor. A bad rule can then produce
a wrong *name*, never a wrong *location*.

**This forbid-list governs contributed rule data, not corpus-derived titles.**
The two are validated at different points by different rules, and conflating them
would reject correct rows: the corpus legitimately returns
`Nemesis: Rogues' Gallery`, whose colon is forbidden on Windows and renders as
`/` in Finder. **Contributed rules are rejected** for containing such characters;
**corpus titles are sanitised** by the preset on the way to a path (§6.8). Data
from a stranger is untrusted input; data from the corpus is trusted content that
needs escaping.

Schema forbids, per contributed field: separators `/` `\`; `.` and `..`; leading `.`;
drive-letter and UNC prefixes; leading `/`; NUL and all C0/C1 controls; tab, CR,
LF; leading `=` `+` `-` `@`; bidi controls `U+202A–202E`, `U+2066–2069`;
zero-width `U+200B–200F`, `U+FEFF`, `U+00AD`; non-NFC input; mixed-script
confusables, since a Cyrillic `е` creates a duplicate series invisible in a
diff; trailing dot or space, legal on APFS and silently stripped on Windows;
Windows device names including with extension — `CON`, `PRN`, `AUX`, `NUL`,
`COM1-9`, `LPT1-9`, `CON.pdf`; `< > : " | ? *`; a per-field byte cap and a total
path budget; rule-count and file-size caps per PR; duplicate keys within or
across files, with a defined merge order.

Enforced in CI on every PR **and** re-validated at `apply`, because the plan
file is untrusted too. Containment via `realpath` per component or `O_NOFOLLOW`, never a string prefix.
`clonefile(2)` additionally accepts `CLONE_RESOLVE_BENEATH` and
`CLONE_NOFOLLOW_ANY`, which enforce containment in the kernel rather than in our
own path arithmetic; prefer them on the clone path.

### 6.12 Implementation language and packaging

**Rust.** This reverses v1 *and* v2's first draft, both of which chose
TypeScript. The owner's scope cut — removing the desktop GUI — removed the
structural argument for TypeScript, and the owner explicitly set aside personal
fluency as a factor.

Why Rust fits this specific tool:

- **`clonefile(2)` is reachable, and this is the strongest argument.** Verified
  by execution: Rust can call it and check its result, giving zero-cost
  copy-on-write copies that are natively no-clobber (§6.10). Node provably
  cannot — `COPYFILE_FICLONE` degrades silently and `COPYFILE_FICLONE_FORCE`
  returns `ENOSYS` on same-volume APFS. That is a capability difference, not a
  preference.
- **The corpus is SQLite.** `rusqlite` with the `bundled` feature compiles the
  SQLite C amalgamation into the binary. **This is a C compile step, not the
  absence of one** — an earlier draft overstated it. It is better-behaved than a
  Node native addon, which was shown not to load from a packaged executable at
  all, but it is a real cost for cross-compiled release artifacts. Without
  `bundled` it dynamically links the system `libsqlite3`, which is fine for a
  Homebrew formula.
- **A static binary with no runtime** simplifies the eventual signed download.

**An argument withdrawn, recorded so it is not reinstated.** An earlier draft
claimed `Result` forcing error handling was a material safety advantage here.
It is not. `Result` is ignorable via `.unwrap()` or `let _ =`, and only
`#[must_use]` plus a deny lint would force it. More importantly it protects
against **none** of the failures that actually dominate this tool: TOCTOU
windows, wrong operation ordering, partial writes, and the `rename()` clobber in
§6.10. The real safety lever is operation ordering and the no-clobber syscall
flags, both language-independent. The lint configuration is still worth setting;
it is not a reason to choose a language.
- **The terminal-UI gap is smaller than assumed.** `indicatif`, `dialoguer` and
  `comfy-table` cover a scan → review → apply CLI without reaching for a full TUI
  framework. The polish bar is reachable.

**What TypeScript would still have won**, recorded so the decision can be
re-examined: the owner's own iteration speed, and a marginally richer interactive
terminal ecosystem in Ink. If frequent hands-on iteration by the owner becomes
the binding constraint, this decision is worth revisiting — it is a close call,
not a landslide.

**Layout: a cargo workspace — a library crate holding all logic, and a thin CLI
binary that calls it.** Two consequences fall out for free, and both are wanted
regardless of any future GUI: the core's public types are `serde`-serializable,
which §6.9's plan format needs anyway, and the core takes a progress callback
rather than printing, which the CLI's own progress reporting needs anyway.

**Release one ships via Homebrew.** A formula satisfies the one-command
constraint, and brew installs are exempt from the Gatekeeper failure that makes
an unsigned downloaded binary die silently. Rust's static binary means the
formula has no runtime dependency to manage.

**Signed downloads are deferred, and the cost is known.** A notarized `.pkg` is
mandatory for a double-clickable download, because a notarization ticket cannot
be stapled to a bare Mach-O. Apple Developer $99/yr. Windows Authenticode
$200–400/yr for OV with a hardware-token key, and OV does not silence SmartScreen
until download reputation accrues; EV, which does, runs $300–700/yr. Verify at
purchase.

**If a desktop GUI is ever wanted**, Tauri's backend is Rust, so the core links
in directly — no sidecar process, no IPC layer, no second implementation — and
the front end is HTML and CSS, unchanged from what an Electron shell would have
needed. A Tauri bundle is roughly 5–10 MB against Electron's ~120 MB because it
uses the system webview. **This is not in the plan and is not being built
toward.** It is recorded only because it is the question the language choice
would otherwise leave hanging.

### 6.13 Command surface

```
comic-renamer scan <source> --dest <dir> --preset komga
                  [--move|--copy] [-o plan.json]
comic-renamer apply plan.json
comic-renamer undo <undo-manifest.csv>
```

**The mode flag belongs on `scan`, not `apply`.** §6.10 selects a per-row
mechanism during `scan`, which is impossible if `scan` is never told the mode —
an earlier draft had the flag on `apply` and the selection at `scan`, which
cannot both be true. So `scan` takes the mode, records `--dest` and the per-row
mechanism in the plan, and `apply` takes no mode flag at all. That is what makes
`apply` reproducible from the plan alone.

**`undo` operates on the undo manifest, not the plan.** They are different
artifacts: the plan is intent, the manifest is a record of what actually
happened (§6.10). Undoing from intent would attempt to reverse operations that
never ran.

---

## 7. Fixture suite

Two layers, because v1 conflated them in one column. **Layer 1 is the resolver's
acceptance test; layer 2 tests the curated map.**

### Layer 1 — title resolution (corpus, no curated data)

| Source filename | Series | Vol | Subtitle |
|---|---|---|---|
| `mazebook.pdf` | Mazebook | — | — |
| `xomnibusvolume1.pdf` | X Omnibus | 1 | — |
| `hellboyomnibusvolume1_seedofdestruction.pdf` | Hellboy Omnibus | 1 | Seed of Destruction |
| `plantsvszombieszomnibusvolume3.pdf` | Plants vs. Zombies Zomnibus | 3 | — |
| `47ronin.pdf` | 47 Ronin | — | — |
| `nemesis_rogues_gallery.pdf` | Nemesis: Rogues' Gallery | — | — |
| `usagiyojimbosagavolume1secondedition.pdf` | Usagi Yojimbo Saga | 1 | — |
| `anthonybourdainshungryghosts.pdf` | Anthony Bourdain's Hungry Ghosts | — | — |
| `cyberpunk2077omnibusvolume1.pdf` | Cyberpunk 2077 Omnibus | 1 | — |
| `thecompleteelfquest_vol1.pdf` | The Complete ElfQuest | 1 | — |
| `maskomnibusvolume1secondedition.pdf` | The Mask Omnibus | 1 | — |
| `umbrellaacademyvolume1_apocalypsesuite.pdf` | The Umbrella Academy | 1 | Apocalypse Suite |

The last two are **disambiguation regression tests**: both resolve to the wrong
title without §6.3 — *MASK Omnibus*, the Kenner toy property, and the French
Delcourt edition respectively.

### Layer 2 — shelf derivation

| Series title | Default folder | `--strip-to-parent` |
|---|---|---|
| Hellboy Omnibus | `Hellboy Omnibus` | `Hellboy` |
| The Complete ElfQuest | `The Complete ElfQuest` | `ElfQuest` |
| X Omnibus | `X Omnibus` | `X` |
| Usagi Yojimbo Saga | `Usagi Yojimbo Saga` | `Usagi Yojimbo Saga` *(identity)* |
| The Goon | `The Goon` | `The Goon` *(identity)* |

### Layer 3 — held-out coverage, and how the suite runs

**Ten fixtures can be passed by ten hardcoded rows, which proves nothing.** v1's
draft compounded this by calling the other 28 reference-bundle files "held out"
when they were the same files already measured.

A real held-out set now exists and is committed as **a text file of filenames
only** — `fixtures/heldout-filenames.txt`, 2,714 names across three publishers,
supplied by the owner. No comics, no binaries, 70 KB.

**The gate is real, and it is measured against ground truth rather than
estimated.** `fixtures/humble-ground-truth.tsv` holds **2,223 verified
filename→title pairs across 38 comic bundles**, extracted from the maintainer's
own Humble Bundle library, where each download is published alongside its real
human-readable title. No hand labelling was required and none is planned.

Measured over all 2,223 rows, with the publisher hint firing on 752 of them:

```
ready                     66.1%
ambiguous -> review        9.6%
subtitle -> review        12.5%
not located               11.8%

of ready rows:
  correct                 99.52%
  WRONG (silent FP)        0.48%   <- the gate

by basis            ready   wrong   error
  unambiguous         742       7   0.94%
  volume-count         70       0   0.00%
  shortest-run         658       0   0.00%
```

**Phase 1 must hold the silent-false-positive rate at or below 0.5% and the
ready rate at or above 66%.** A ready-rate regression is a disappointment; a
silent-wrongness regression is a defect that blocks merge.

**Grade carefully — most apparent errors are grading artifacts.** A first pass
reported 2.65% and all ten sampled failures were the grader's fault: Humble
writes `&` where the corpus writes "and", prefixes promotional items with
"Free Issue" including a zero-width character, and inserts `Vol 1:` mid-title.
Comparison must expand punctuation on **both** sides, strip promotional
prefixes and embedded volume markers, and accept a match against any such
normalisation. The real rate is 0.48%, five times lower than the naive one.

`fixtures/label-sample-200.txt` remains as a 200-row spot-check for a human who
wants to audit the automated grading; it is no longer on the critical path. Once that exists, the gate becomes real
and the rule stands: **a located-rate regression is a disappointment; a
silent-false-positive regression is a defect that blocks merge.** Until it
exists, no claim about silent wrongness may be made in the README, the PR
template, or §6.5's corpus-bump check.

**Execution tests use generated files, never real comics.** A fixture generator
materialises zero-byte files from the committed name list, so the suite runs in
CI, on Rob's machine and on any contributor's, and consumes no disk. This is
sound rather than a compromise: the resolver is filename-only by contract
(§5), so a zero-byte file with the right name is a *complete* input.

For the paths that genuinely need bytes:

- **Collision, case-folding, NFC/NFD and quote-normalisation tests:** tiny files,
  a few bytes each.
- **Free-space preflight and the streaming path:** a sparse file created by
  `seek`-then-write, which reports a large `st_size` with `st_blocks` non-zero
  (measured: 32). Costs no meaningful disk and exercises the real code.
- **Placeholder detection:** a **separate** file with `SF_DATALESS` set.

**These must be two different files.** An earlier draft said one sparse file
served both; it cannot. A `truncate`-only file reports `st_blocks=0`, a
`seek`-then-write file reports `st_blocks=32`, and under the discarded
`st_blocks`-based predicate `scan` would have flagged the streaming fixture
itself as dataless.

**Evaluation input is distributor bundles only.** An earlier pass admitted a
scene release into the set and then drew a scope conclusion from it; that is
circular and the finding was withdrawn.

---

## 8. Phases

| Phase | Deliverable | Done when |
|---|---|---|
| **0 — Repo harness** | Agent and contributor instructions, confirmed docs layout, CI skeleton. Runs **before** anything else in this spec. See §10.1. | A fresh agent or contributor can read one file at the repo root and know the invariants, where docs live, and how to run the suite. |
| **0a — Komga convention** | Just enough of the app research to author one preset correctly. Prerequisite for Phase 1, small. | Komga's folder and filename rules are documented with sources. |
| **0b — Full app spike** | All 13 candidate apps (§9). Runs **in parallel** with Phase 1; gates Phase 2 only. | Each app rated: documented or reverse-engineered, filename/folder/metadata-driven, preset effort, blockers. |
| **1 — Core + Komga** | Resolver (corpus + disambiguation + residue), Komga preset, `scan`/`apply`/`undo`, **plan file only**, move-by-default with the full §6.10 safety set, and a **minimal Layer B override file** the user edits by hand. Ships via Homebrew. | Layer-1 fixtures pass; held-out located-rate ≥ 92.3%; `scan` → review → `apply` → `undo` completes against generated fixtures including both sparse cases and every collision class; and every mechanism refuses an existing destination at the syscall. |
| **2 — Breadth + polish** | Kavita and Panels presets *if 0b validates them*; interactive review walk sharing one contract with the plan file. | The preset abstraction survives every app the spike validated. If fewer than two survive, Phase 2 is **re-scoped, not failed** — substitute the highest-rated app from 0b's wider list (§9) rather than shipping a preset for an app that cannot use it. |
| **3 — Curated map + contributions** | `--strip-to-parent`, the line→parent map, §6.11 schema with CI enforcement, PR emission from a corrected plan. **Repo goes public here.** | An outside contributor lands a map row without touching code. |
| **4 — Live lookup** | Optional BYOK Metron and Comic Vine for corpus misses. | Corpus-miss rows get a candidate. Tool still fully works with no key. |

**Cut from Phase 1 deliberately**, to get to a working tool sooner: `--hardlink`
(clone dominates it and cloud-sync clients mishandle links); `--resume` and the
operation journal (the undo manifest already provides recovery, and an
interrupted run can simply be re-run); the interactive review walk and the
two-surface threshold, which move to Phase 2; and the live-lookup plumbing in
§6.6 entirely.

**Added to Phase 1 against the earlier draft:** a minimal Layer B override file.
An earlier draft had Phase 1 shipping "no curated data," but §6.2.1 establishes
that Layer B is the only place a wrong *title* can be corrected — so that
version shipped anti-goal §4's "a corpus nobody can correct" by construction. It
needs no interface, just a documented file format and a loader.

**Backlog, with open questions attached.** PDF→CBZ conversion — load-bearing
rather than cosmetic, because **Kavita files every file with no issue number as
a "Special"** and the entire corpus has no issue numbers, so Phase 1 can succeed
into an app that will not shelve its output correctly; open questions are
double-page spreads, resolution and output size. Signed binaries and
single-executable packaging (§6.12). Mylar, Calibre and Ampchor presets —
Ampchor's rules exist and the owner can supply them, but it is in private beta.
Folder watching.

---

## 9. App research spike

Per owner instruction, researched deeply. **Launch:** Komga, Kavita, Panels.
**Also:** Mylar, Calibre, Ubooquity, YACReader, ComicRack, Chunky, Paperback,
Suwayomi, Plex/Kometa, Ampchor.

**"Validated" has a definition, so Phase 2's gate is not a judgement call.** An
app is validated when (a) its convention is documented or reproducibly
reverse-engineered, (b) it is filename- or folder-driven such that renaming
changes what the app displays, and (c) a file with no issue number — our entire
corpus — is shelved as a normal entry in its series. **Kavita fails (c) on
current evidence**, filing every issue-number-less file as a "Special". If that
holds, Kavita is *not* validated, and the choice is to promote PDF→CBZ out of the
backlog or drop the preset.

Per app: is the convention documented or reverse-engineered, with sources? Is it
filename-, folder-, or metadata-driven — and if metadata-driven, is renaming
meaningful at all? Exact folder structure and filename grammar. **Collected
editions specifically** — how are they distinguished from single issues? That is
the actual shape of our data and is frequently undocumented. Forbidden
characters and sort behaviour. Supported formats and behaviour on an unreadable
one. Preset effort and blockers.

Known in advance: **Kavita's Specials behaviour** (§8 backlog) is the single
most important thing the spike must confirm or refute. **Ampchor publishes no
convention** — it performs its own metadata matching, is in private beta, and
supports CBZ/CB7/CBT but not CBR. **Calibre** manages its own directory layout
on import and may not be renameable-toward at all.

### 9.1 Data source terms — settled

| Source | Bundle a derived corpus? | Terms |
|---|---|---|
| **GCD** (via paperbunkr extract) | **Yes** | CC BY-SA 4.0. Attribution to the Grand Comics Database™. ShareAlike binds the data, **not the software source**. |
| **Metron** | Yes | CC BY-SA 4.0, commercial use explicit, ShareAlike data-only. API 20/min, 5,000/day, **each user needs their own account**. |
| **Comic Vine** | **No** | Non-commercial only; forbids reproducing the data in another form; revocable at discretion. 200/resource/hour plus an undocumented 420 throttle. Live opt-in only. |

**Why the corpus must ship inside the tool:** every provider is
bring-your-own-key for live lookup, so no API can resolve a first run for a
stranger with no credentials. Cover images are excluded from all three licences
— irrelevant, we want titles.

---

## 10. Repository conventions

```
docs/specs/     design documents
docs/plans/     implementation plans
docs/research/  spike and experiment output
```

Plus `fixtures/` for committed test data — filename lists only, never comics.

Dated `YYYY-MM-DD-<topic>.md`. Deliberately generic and tool-agnostic, so a
stranger reading the public repo needs no context to navigate it.

### 10.1 Agent and contributor harness (Phase 0)

**`docs/` is the right home for specs, plans and research — keep it.** But it is
not sufficient on its own, because **nothing reads `docs/` automatically.** An
agent opening this repository reads the root, and a contributor reads the README.
Both need the invariants before they touch anything.

**`CLAUDE.md` at the repo root is canonical.** `AGENTS.md` is a short pointer to
it, so contributors using other tooling land in the same place rather than
drifting into a second set of instructions. One source of truth, two entry
points.

**The load-bearing content is the invariants, not a project tour.** The dominant
failure mode for an agent in this repository is not ignorance — it is a
well-intentioned change that quietly violates a safety property discovered the
hard way. Every item below was found during review or measurement, and each one
is the kind of thing a competent contributor would otherwise undo:

1. **Presets are the only code that constructs paths.** Rule data never contains
   a path, a separator, or anything path-shaped (§6.11).
2. **`apply` recomputes destinations from metadata.** It never trusts a path
   string found in a plan file (§6.9).
3. **Size zero is never an error.** An "empty means corrupt" guard silently
   disables the entire fixture suite (§6.10).
4. **Never commit the corpus binary.** Bump the manifest (§6.5).
5. **The resolver never opens a file.** Filenames only, always (§5).
6. **Collision comparison is NFC-normalised and case-folded**, never a literal
   path string, and emitted paths are always NFC (§6.10).
7. **A located-rate regression is a disappointment; a silent-false-positive
   regression is a defect** and blocks merge — **once the labelled subset exists.
   Until then the silent-wrongness gate is unmeasured and must not be claimed**
   (§7).
8. **Evaluation input is distributor bundles only** — not scene releases, not
   hand-renamed files (§7).
9. **Tests use generated files, never a maintainer's library** (§7, D12).

It should also carry: how to run the suite, the commit and PR protocol, and the
rule that a corpus-manifest bump requires the coverage delta in the PR body.

**CI at Phase 0 is a skeleton, not a full pipeline** — enough to run the held-out
coverage measurement and the rule-data schema validation, so both exist before
there is code to regress.

*(Note for the owner: the installed `oh-my-claudecode` toolchain has a `drydock`
skill that scaffolds exactly this four-pillar harness. Worth running rather than
hand-writing, then editing the invariants above into it.)*

---

## 11. Open questions

**11.1 — Residual ambiguity.** 4.7% of held-out rows stay ambiguous with no
publisher inferable. §6.3 now says such a row stays `review`. Open: is 4.7%
acceptable as a review rate, or does it justify prompting the user for a
publisher once per run — a single question that would collapse most of it?

**11.2 — Panels.** Launch roster keeps it, but it may be metadata-driven, in
which case renaming for Panels is near-pointless. Spike 0b decides. If it falls,
does a third app substitute or does Phase 2 pass on two?

**11.3 — Resolved.** Manifest-pinned, fetched and hash-verified on first use,
never committed, refreshed by a gated monthly pull request. See §6.5.

**11.4 — Source tree shape.** Every fixture is a flat `.pdf`. Real bundles ship
PDF + CBZ + EPUB per title in nested folders alongside non-comic extras. Does
Phase 1 recurse, and how are multi-format duplicates handled — one logical item
with a preferred format, or three rows? This is Phase 1 architecture and the
fixture suite hides it.

**11.5 — Plan eligibility. Phase-1 blocking, not deferrable** — see §6.9's
`ready`-rate estimate. Does `apply` refuse every row still marked
`review`, or execute everything not `failed`? An assistant that mass-flips
statuses satisfies §6.9's letter and defeats its purpose.

**11.6 — `birdkingvolume1`** matched series *Birdking*. Human check against the
Dark Horse edition before trusting that fixture row.

**11.7 — Manga corpus coverage.** *Domestic Girlfriend* is unfindable across six
held-out files, suggesting GCD romanisation gaps for Kodansha titles. Measure
coverage on manga specifically before Phase 2 promises it; Layer B overrides may
be the answer, in which case manga carries a curation cost the rest does not.

---

## 12. Left to the builder

- Cargo workspace layout: the core library crate's public API surface, and the
  progress-callback seam the CLI's progress reporting needs.
- Plan file format and schema (§6.9 rules it out being TSV, does not pick a
  replacement).
- Rule-data file format and its `serde` model plus validation, within §6.11's
  constraints.
- The visual design of the plan-file review flow. The crate choices are named in
  §6.12; the design is not.
- Undo-manifest field set beyond the four in §6.10.
- Which `clippy` lints are denied, including whether `must_use` results are
  errors (§6.12).
- Test strategy beyond the fixtures, including the held-out corpus in §7.

---

## 13. Decision log

Carried forward from v1 with its reasoning intact, because the *why* outlives
the sections.

**D1 — Execution defaults to move.** 31.2 GiB payload against 11.0 GiB free made
copy-by-default unable to complete. Same-volume move is also safer against
interrupted-run corruption. §6.10.

**D2 — Case and Unicode collision detection is a Phase 1 blocker.** Demonstrated
on this project's own APFS volume. Under move-by-default it destroys the only
copy. §6.10.

**D3 / D9 — MIT for code, CC BY-SA 4.0 for the corpus.** Replaces the repo's
inherited GPL-3.0, which foreclosed Mac App Store distribution and blocked
relicensing once outside contributors hold copyright. CC0 is unavailable because
every viable source is share-alike. `LICENSE` already replaced; copyright holder
"Business Casual" — adjust if a different entity is preferred. Outstanding when a
corpus directory exists: `data/LICENSE` and the README attribution line.

**D4 — v1's resolver architecture was wrong; corpus lookup replaces filename
parsing.** Confirmed empirically, 40/40. §1.2, §6.1.

**D5 — Review findings live in `docs/research/`**, not inlined here.

**D6 — Launch roster stays Komga, Kavita, Panels, contingent on spike 0b.**
Phase 2's exit criterion reworded to "every app the spike validated, minimum
two," because the spike may invalidate two of the three.

**D7 — Homebrew, unsigned, for release one.** $0/yr versus $300–500/yr, and brew
installs dodge the Gatekeeper failure. Removes the single-executable requirement
from Phase 1 entirely. §6.12.

**D8 — Repository goes public at Phase 3**, when the contribution flow needs it.
Reinforces that the bundled corpus, not contributions and not a live API, has to
make a first run usable.

**D10 — Series folder defaults to the published title; `--strip-to-parent` is
opt-in.** Reverses v1's strip-by-default. The curated map stops gating Phase 1,
and the owner's preference becomes a flag rather than a prerequisite. §6.2.

**D11 — Disambiguation by language and publisher is mandatory.** Measured 7.5%
silent-wrong rate without it, including resolving *The Mask Omnibus* to a Kenner
toy property. Publisher is inferred free from the source folder name. §6.3.

**D12 — Tests use generated files, never the owner's library.** Owner's call. A
suite depending on a personal Dropbox cannot run in CI, on Rob's machine, or for
a contributor — a worse problem than the disk cost that prompted it. The resolver
is filename-only by contract, so zero-byte files are complete inputs; sparse
files cover the free-space and placeholder paths at no cost. Resolves the
contradiction where Phase 1's acceptance criterion required a 31.2 GiB run that
§6.10's own safety rules forbid. §7.

**D13 — There are two curated data layers, not one.** Layer A maps
collected-edition lines to parent shelves; Layer B overrides filenames the corpus
cannot reach. Layer B is required rather than optional: initialisms like
`TWD_CompendiumOne` are the floor under the residual 7.7%, and Layer B is the
only place a wrong *title* can be corrected. §6.2.1.

**D14 — `apply` recomputes destination paths from metadata and never trusts the
plan's stored path.** The plan is handed to AI assistants and attached to public
PRs, so trusting a path string in it would reintroduce §6.11's threat model one
layer up. "Edit" therefore means editing metadata, not paths. §6.9.

**D15 — Subtitles are not recovered in Phase 1.** No corpus field holds them;
residue-as-series-name fails because the corpus key is a *suffix* of the match,
reachable only by substring search that would discard the boundary guard that
prevents silent false positives. Dictionary segmentation works but loses
apostrophes. Phase 1 carries the unsplit residue and marks the row `review` —
9.8% of held-out files. No fixture asserts a split subtitle. §6.4.

**D16 — Punctuation-expansion variants are part of the index.** Distributors
transliterate `/`, `+` and `&` into words, inconsistently within one catalog.
Worth a measured 3.4 points. §6.1.

**D17 — Rust, reversing both earlier drafts.** Owner's call: efficiency and
fitness outrank personal preference. The owner's own scope cut (D18) removed
TypeScript's structural advantage, and three things favour Rust specifically —
`rusqlite` statically links the corpus database where Node needs a native addon
that will not load from a packaged executable; `Result` makes an unhandled
filesystem error a compile error, which maps onto the project's dominant risk;
and a static binary with no runtime simplifies the eventual signed download. A
close call, not a landslide — recorded as revisitable if the owner's own
iteration speed becomes the binding constraint. §6.12.

**D18 — No desktop GUI in the plan.** Owner's call: the Mac app was an
aspiration, not a commitment, and assuming it distorted the architecture. The
former desktop-GUI phase is deleted. The core remains a library crate with a thin CLI binary — not to keep
a GUI door open, but because that is what makes it testable. If a GUI is ever
wanted, Tauri's Rust backend links the core in directly, which is a better path
than the TypeScript plan it replaced.

**D19 — The corpus is manifest-pinned, never committed.** Owner asked for a
monthly refresh; committing a 33.7 MB SQLite file twelve times a year would add
~400 MB of permanent history for a file regenerated upstream. Instead a scheduled
workflow opens a manifest-bump PR, gated on the held-out coverage delta and a
human merge, because a corpus bump changes resolution for every user. Resolves
open question 11.3. §6.5.

**D20 — Every run writes an undo manifest incrementally.** Owner's requirement, a
two-column original→final record the tool can reverse. Written and flushed
*before* each operation rather than after, because the failure that matters is an
interrupted run leaving files moved with no record. RFC 4180 quoted, since comic
titles contain commas, with formula-injection prefixes defused. §6.10.

**D21 — Size zero is never an error.** Stated explicitly in response to the
owner's question about whether size checks would break zero-byte fixtures. They
do not — all three checks behave correctly on an empty file — but the rule is
written down so that a future "empty means corrupt" guard cannot silently disable
the suite. §6.10.

**D22 — A repo harness phase precedes the spec.** Owner's call. `CLAUDE.md`
canonical at the root with `AGENTS.md` pointing to it, because `docs/` is correct
but nothing reads it automatically. Its load-bearing content is the nine
invariants in §10.1, each of which a competent contributor would otherwise
reasonably undo.

**D23 — Resolved by measurement: cloning works through a cloud-sync provider but
hydrates the source, so it does not make `--copy` free in general.** The test:
`clonefile` on a real Dropbox placeholder succeeded and left the source
materialised (`blocks=0, SF_DATALESS` → `blocks=384, flags cleared`); the same
call against an already-local file in the same tree moved `df` by 16 KB for a 6 MB
file.

So copy-via-clone is free **only when sources are already local** — true of the
reference bundle today, not true of a stranger's online-only library, where it
would pull the whole payload over the network first.

**Move stays the default**, since the measurement did not deliver the
unconditionally-free copy that would have justified flipping it. Two requirements
fall out instead: the pre-flight must disclose **download** cost alongside disk
cost, and `SF_DATALESS` detection at `scan` is load-bearing rather than
informational. Whether to flip the default for the already-local case remains the
owner's call, now with real numbers. §6.10.

**D24 — No-clobber is enforced at the syscall, per mechanism.** A collision check
followed by `rename(2)` is not a no-clobber guarantee, because `rename` silently
overwrites. `renamex_np(…, RENAME_EXCL)` on the move path, `clonefile`'s native
`EEXIST`, `O_EXCL` on the stream, `link(2)`'s own failure. Without this, D1's
move-by-default is a route to D2's data loss. §6.10.

**D25 — `SF_DATALESS` is the dataless predicate.** `st_blocks == 0 && st_size >
0` is wrong and is withdrawn: a `truncate`-created sparse file reports
`st_blocks=0` with `st_flags=0`, so the old predicate misclassified legitimate
sparse files — including this project's own streaming fixture. §6.10, §7.

**D26 — The silent-false-positive gate is unmeasured and may not be claimed until
a labelled subset exists.** Building it (~200 `ready` rows, verified against
title, publisher and year) is a Phase 0 deliverable. The located-rate of 92.3%
*was* measured and stands. §7.

**D27 — Phase 1 is cut, and gains one thing.** Out: `--hardlink`, `--resume` and
the journal, the interactive walk and its threshold, and all live-lookup
plumbing. In: a minimal hand-edited Layer B file, without which Phase 1 violates
anti-goal §4 by construction. §8.

**D28 — Ambiguity is measured over corpus records, not title strings.** Three
different books named *Velvet* are one title and three records; comparing titles
calls that unambiguous and then guesses. Measured: counting titles inflates
`ready` from 40% to 79%, every inflated row a silent guess. Also withdraws
"prefer the earliest edition," which picked Malibu 1993 over Image 2013 inside an
Image bundle. §6.3.

**D29 — Two new disambiguation signals, one hard and one on probation.** Volume
feasibility — a volume-N file cannot belong to a series with fewer than N entries
— is a hard constraint. Shortest-feasible-run is a heuristic that lifts `ready`
from 31.7% to 71.1% and is therefore **not permitted to promote a row to `ready`
until the labelled sample measures its error rate.** A 39-point swing resting on
an unproven prior is the shape anti-goal §4 exists to prevent. §6.3.

**D30 — The publisher hint is worth less than first measured.** The 40-file
reference bundle suggested it was decisive (5 ambiguous → 1). Across 2,714
held-out files it resolves 7.4% and leaves 28.9% ambiguous within a single
publisher. Still worth having, no longer load-bearing. §6.3.

**D31 — Ground truth comes from the maintainer's Humble library, not hand
labelling.** Humble's order API publishes each download beside its real
human-readable title, so a filename→title mapping for every owned book is
directly extractable. 2,223 comic rows across 38 bundles, committed as
`fixtures/humble-ground-truth.tsv` — bundle, title and filename stem only, no
URLs, prices or account data. This retires D26's 40-minute hand-labelling plan
and replaces a 200-row sample with a library-scale verified set. Non-comic
bundles (tabletop RPG rulebooks, prose fiction, O'Reilly technical titles) are
filtered out; one bundle of mixed prose and comics was dropped whole rather than
contaminate the set, and one RPG-branded bundle that is actually Dynamite comics
was kept. §7.

**D32 — The shortest-feasible-run heuristic is validated and off probation: 0
errors in 658 graded rows.** D29 held it back as an unproven prior deciding more
rows than everything else combined. Measurement cleared it as the lowest-error
basis of the three. The `ready` rate is 66.1% with it and roughly 36% without.
Re-validate on every corpus bump. §6.3.

**D33 — The language filter must never fall back to another language.** Six of
the seven total errors across 2,223 rows came from silently accepting a record
in another language when no English one matched, resolving `noragami_vol1`–`vol6`
to the Japanese original and producing a Japanese-script folder name. No match
in the requested language means `review`. §6.3.

**D34 — The Komga preset is specified, and Komga is validated.** Researched
against source rather than documentation. It passes §9's validation test that
Kavita fails: a volume-numbered file with no issue number is an ordinary book and
there is no "Special" bucket. Three findings shape the preset: Komga has **no
filename parser at all**, so for PDFs our output is the entire metadata record;
`Series.name` is the leaf folder only, so emit flat `<Series>/<Book>.<ext>`; and
numbering is **positional** after a natural sort, so filename sort order is
reading order and volume must precede subtitle. Zero-padding is confirmed
unnecessary for Komga, though possibly still wanted for other tools — owner's
call. §6.8.1.

**D35 — A Komga pre-flight check is required before renaming into an existing
library.** A rename is delete-plus-add; Komga restores read progress, read lists
and collections only by matching file size then hash, and **hashing enabled after
the fact cannot help** — a deleted record can never acquire a hash because
hashing reads the file's current path. `emptyTrashAfterScan` purges the records
outright. Assert `hashFiles === true` and `emptyTrashAfterScan === false` via
`GET /api/v1/libraries` and **refuse**, not warn. Both defaults are already safe,
so this only bites users who changed them — silently, which is the problem.
§6.8.2.

**D36 — Correction: NFC/NFD does not split a Komga series in two.** An earlier
draft claimed identical data would produce different libraries on macOS and on a
Linux server. Komga compares through ICU collators with
`CANONICAL_DECOMPOSITION`, so the forms are equal. The demonstrated APFS
case-folding and normalisation *collision* on the filesystem is unaffected and
still a Phase 1 blocker (D2); NFC output remains a hard invariant. §6.10.

**D37 — Volume numbers are zero-padded after all, to a per-series width.**
Owner's call, reversing the earlier unpadded choice. Komga genuinely does not
need it — its comparator reads digit runs numerically — but `ls`, Finder, every
file manager and most scripts sort lexically, and under those `Vol. 10` precedes
`Vol. 2`. Width is the digit count of the series' highest volume, read from the
corpus `issue_count` the resolver already has, floored at two. Per-series rather
than fixed, because a fixed two digits breaks silently at volume 100 and manga
runs pass it. §6.8.1.
