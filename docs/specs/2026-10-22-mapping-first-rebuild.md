# Mapping-first rebuild

Status: **approved 2026-10-23.** Builder-checked 2026-10-22; owner decisions
recorded 2026-10-22 and 2026-10-23. Amended after real files from three further
marketplaces (§5.3.1) and the Kavita source research in
`docs/research/2026-10-23-kavita-from-source.md`.
Supersedes parts of `2026-10-19-comic-renamer-design.md` — see §2.
Date: 2026-10-22

---

## 1. Intent

Comic Renamer currently *derives* each comic's title by matching its filename
against a 1.2-million-series GCD corpus. That was the wrong shape for the
problem.

The files this tool exists to rename are bought from comic marketplaces —
Humble Bundle, Fanatical, Global Comix, Sweet Shop. **The marketplace
publishes the real title next to every download.** The title is a known
quantity. We were throwing it away and then paying a large, fragile machine to
guess it back.

Worse than wasteful: for some marketplaces it is impossible. Fanatical ships
`HackSlash Back to School.cbz` for *Hack/Slash: Back to School* — the `/` and
`:` are gone from the input, so no resolver can recover them (§5.3.1).

### 1.1 What the guessing actually costs, measured

`cargo xtask measure`, 2026-10-22, against the implemented resolver and the
2,223 verified rows in `fixtures/humble-ground-truth.tsv`:

```
   1172   52.7%  ready
    430   19.3%  review - ambiguous
    290   13.0%  failed - no series matched
    238   10.7%  review - unsplit subtitle
     67    3.0%  review - volume infeasible
     26    1.2%  review - no language match

located   91.6%   gate >= 92.3%   FAIL
ready     52.7%   gate >= 66.0%   FAIL
silent FP 10.12%  gate <= 0.48%   FAIL
```

Every gate fails. The silent false-positive rate — §4's "confident wrongness"
anti-goal, the one the design calls the one that matters — is **21x its
target**, and 0.48% was never re-measured against the implemented resolver at
all. Folding case in the grader takes it to 4.23%, still 9x, and case folding
was rejected on 2026-10-22 because it would hide real mismatches.

### 1.2 Why a lookup is not merely better but exact

The same filenames, keyed by a normalised stem (§5.3) rather than resolved:

```
usable rows 2223   distinct titles 1703   distinct stems 2055
normalised keys 2036   keys mapping to >1 title: 0  (0.00%)
```

**Zero ambiguity.** The 430 rows the resolver calls "review — ambiguous" are
ambiguous only because the answer was discarded before the search began. In
the actual input domain, filename → title is a function.

### 1.3 The reframe

This is not a generic renamer. It is a renamer for files bought in
distributor bundles, where titles are known and recorded. Sorting for Komga,
Kavita and friends is a convenience layered on top.

**An unmapped filename means nobody has mapped it yet.** It is a gap in a
contributed dataset, not a failure of inference. The tool's job in that
moment is to make filling the gap easy and to offer its best guess as a
starting point.

---

## 2. What this supersedes

| In the 2026-10-19 design | Now |
|---|---|
| §6.1 Stage ① title resolution | Demoted. Resolution produces *suggestions* for the mapping session only. Never authoritative. |
| §6.3 Disambiguation, "mandatory, not a refinement" | Deleted as a decision procedure. Candidates become a picker's menu. |
| §6.7 Status model (`ready`/`review`/`failed`) | Replaced by `mapped` / `unmapped`. See §6.2. |
| §7 Fixture suite, the silent-false-positive gate | Dissolved, not re-derived. See §8. |
| §6.5 Corpus distribution | Retained, but the download becomes lazy and optional. See §5.6. |
| §6.11 Rule-data safety | Unchanged in content, promoted from Phase 3 to load-bearing on day one. See §7.3. |

Unchanged and not to be touched: §6.8 presets, §6.9 the plan, §6.10 execution
and safety, §6.13's `scan`/`apply`/`undo` contract, and all nine invariants in
`CLAUDE.md`.

---

## 3. Goals

1. A mapped file renames deterministically, with no inference and no corpus,
   whichever marketplace it came from.
2. An unmapped bundle is announced plainly, then mapped in one interactive
   session with guesses pre-filled.
3. A user's corrections persist locally and can be contributed upstream in one
   step.
4. Every filesystem guarantee the current tool has survives intact.

## 4. Non-goals and anti-goals

Inherits all of §3 and §4 from the 2026-10-19 design. Additions:

**Non-goals**
- Mapping files that did not come from a distributor bundle. Scene releases
  and hand-renamed libraries stay out of scope (invariant 9).
- Guess quality as a product promise. Guesses are a typing aid.

**Anti-goals**
- **Storing anything the user did not approve.** The old silent-FP risk is
  removed by construction: nothing enters the mapping without a keystroke.
- **A mapping table whose keys can silently change.** See §5.3 — key
  normalisation is effectively a hash of the filename, and altering it
  invalidates every row ever contributed.
- **Contributed data that can influence a path.** §6.11 exists for this and is
  now live.

---

## 5. The mapping

### 5.1 Row shape

A row carries metadata fields only — never a path, never a separator
(invariant 1, §6.11).

```
key          normalised filename stem (§5.3)
series       display string, the Komga series folder
volume       integer, optional
subtitle     optional, e.g. "The Original Quest TPB", "The Special Edition"
lang         informational only — see below
volume_count series' known highest volume — see §5.5
bundle       provenance, and the lookup tiebreaker — see §5.4
```

**`subtitle`, not `edition`.** `Resolved` and `preset/komga.rs:55` already have
`subtitle` and render it as ` - Subtitle`. An earlier draft of this document
invented `edition` and dropped `subtitle` while also promising §6.8 presets
would not be touched; those cannot both hold. Keeping `subtitle` holds the
promise and avoids the collisions below.

Measured cost of getting this wrong: with series and volume alone, **280 of
2,223 titles (12.6%) share a destination with at least one other distinct
title**, in 49 groups — all three of `Usagi Yojimbo: 35 Years of Covers`,
`: 40th Anniversary Reader` and `: The Special Edition` collapse onto one
path. Invariant 6 then refuses them, so they would be `mapped` and still
unplaceable, which is strictly worse than today's `review`.

**`lang` is informational.** Lookup never consults it (§5.4), so `--lang` and
D33's hard no-fallback filter govern *guessing* only. On a mapped row the
field records what the corpus thought at mapping time and nothing reads it.

No field may contain a tab, which is what makes the storage format below safe.

### 5.2 Format and location

**Format: TSV, one row per line, header row first.** Chosen because the table
is already ~1,900 rows and will grow; a TSV diff is reviewable line-by-line in
a PR; contributors can edit it in a spreadsheet; and §6.11 already forbids tab
in every contributed field, so the format needs no quoting rules at all. TOML
becomes unreadable at this row count and JSON triples the diff noise.

Two layers, local winning:

| Layer | Location | Loaded how |
|---|---|---|
| Shipped | `data/mappings/*.tsv` in-repo | `include_str!`, embedded in the binary |
| Local | `$XDG_CONFIG_HOME/comic-renamer/` else `~/Library/Application Support/comic-renamer/` | read at runtime |

**The local file goes in Application Support, not Caches.** The existing
`cache_dir()` points at `~/Library/Caches`, which is correct for a
re-downloadable corpus and wrong for user data — macOS may evict it. These are
different directories for different reasons and the builder should not reuse
`cache_dir()` here.

Shipped mappings are embedded because an installed binary run from an
arbitrary directory must still have its data. This repo has already shipped
that defect once, with `Overrides::load(Path::new("data/overrides.toml"))`
resolving against the cwd.

Local overrides shipped, for the same reason Layer B outranks the corpus in
§6.2.1: the local file is the only place a user can correct a wrong row.

### 5.3 Key normalisation — minimal, and therefore safe to freeze

```
1. NFC
2. lowercase
3. strip all non-alphanumerics
```

That is the whole rule. **2,036 keys over 2,223 Humble rows, zero
collisions.**

#### Why it is this and not something cleverer

An earlier draft stripped Humble's download-id digit runs and `_hd` quality
tokens, giving 1,846 keys. Real files from the other three marketplaces
(§5.3.1) killed that rule, and the trade was never good anyway:

```
frozen (aggressive)         1846 keys   0 collisions
minimal (NFC+lower+alnum)   2036 keys   0 collisions

cost of dropping the strip rules: 190 extra rows out of 2036  (9.3%)
```

190 rows in a generated, contributed table is nothing. A frozen rule that is
wrong for half the marketplaces is unrecoverable — and D40 freezes it, so the
migration it forbids would have been needed on day one.

There is a deeper reason. **Deciding which parts of a filename are noise is
itself inference**, which is the thing §1.3 removes. `_hd` looks like a quality
marker until a series is actually called that. Minimal normalisation makes no
claim about meaning: two spellings of a filename are two rows that happen to
give the same answer, and that is fine.

#### What this means for a variant

`xo_manowar_vol1_1410891143` and `xo_manowar_vol1_hd_1410891143` are two rows
with identical fields. The seeding generator emits both; a contributor mapping
one is prompted for the other. Duplication in a data file, zero risk in a
frozen rule.

#### Still frozen, and still a migration

The rule is a one-way function from filename to key. Changing it invalidates
every row ever contributed. A pinning test asserts 2,036 keys against the
committed fixture, and any change is a breaking data migration with a version
bump — not a refactor.

### 5.3.1 What the four marketplaces actually name files

Measured against real samples, 19 files across three marketplaces beyond
Humble. **Every marketplace bakes different noise into the filename, which is
why §5.3 strips none of it.**

| Marketplace | Shape | Noise it bakes in |
|---|---|---|
| Humble | `valiantmasters_bloodshot_hd_1410912611` | quality token, download id |
| Fanatical | `HackSlash Back to School` | none — but see below |
| Global Comix | `hammer-head-1_release-0001_978-1-5458-3063-5` | release counter, **ISBN-13** |
| Sweet Shop | `young-shadow-the-watchdogs-graphicnovel` | type suffix: `issue-N`, `anthology`, `graphicnovel` |

Two of these defeat the aggressive rule outright:

- **Global Comix hyphenates its ISBN**, so `\d{9,}` never sees nine
  consecutive digits and the ISBN lands in the key:
  `hammerhead1release00019781545830635`. A different printing is a different
  ISBN is a different key for the same comic. `release-0001` is a revision
  counter that moves on re-release, changing the key again.
- **Sweet Shop's type suffix is not a title**, and no Humble-derived rule
  removes it.

#### Fanatical proves the premise

```
HackSlash Back to School.cbz   ->  Hack/Slash: Back to School
Dracula Motherfker.pdf         ->  Dracula, Motherf**ker!
68 Vol. 6 Last Rites.cbz       ->  '68 Vol. 6: Last Rites
```

**The marketplace already stripped `/`, `:`, the apostrophe and the profanity.
The title is not recoverable from the filename by any amount of inference.** No
resolver, however good, gets `Hack/Slash` back from `HackSlash` — the
information is gone from the input.

This is the strongest evidence for §1.3, and it independently confirms D47: the
characters the marketplace strips are exactly the ones §6.11 was arguing about,
so a table that stores only a sanitised form could never hold the real title.

#### One key per title, across formats

19 files produced 12 keys. `.cbz`, `.pdf` and `.epub` of one comic share a
stem, so **one mapping row serves every format** — `PlanRow` already carries
`extension` separately, and the three file into one series folder under one
name with different extensions. No collision, no duplicate mapping work. This
was not designed for and it falls out correctly; the builder should pin it with
a test rather than rediscover it.

#### Guess source is per-marketplace

The corpus is the wrong guess engine for Fanatical, whose filename is already
the title modulo sanitisation; title-casing the stem beats a GCD lookup there,
and the same holds for kebab-case Global Comix and Sweet Shop names. So the
guess layer dispatches on marketplace, with the corpus as one strategy among
several and "no guess" always permitted (§5.6).

Marketplace is recorded per mapping file, not inferred from a user's folder
names.

### 5.4 Lookup, and why `bundle` is a tiebreaker rather than part of the key

Lookup is `key → rows`. One row: that is the answer. More than one row:
`bundle` disambiguates, and a source folder that matches no bundle leaves the
file unmapped rather than guessing.

This is deliberately different from both the draft and §6.2.1. The draft
removed `bundle` from lookup entirely on the strength of the zero-collision
measurement, which reverses §6.2.1's explicit warning that a bare filename key
recurs across publishers. But §6.2.1's examples — `vol1.pdf`, `001.pdf`,
`cover.pdf` — are substantially already filtered by the ancillary rules in
`scan.rs`, which is part of why the measurement saw no collisions.

Making `bundle` a tiebreaker rather than a key component gets both properties:
the common path is a single stem lookup with no folder-name dependency, and a
genuine collision from a second distributor is a *new row*, not a breaking
change to a frozen key rule. Without this, D40's freeze would make the first
cross-distributor collision unrecoverable.

Note that 158 of the 2,036 keys already appear under more than one bundle, and
in all 158 the title is identical — so the CI gate must compare *fields*, not
merely count rows per key, or it fires on correct data.

### 5.5 `volume_count`, and the D37 problem this solves

D37 pads volume numbers to a per-series width taken from the corpus
`issue_count`. With the corpus no longer consulted on the mapped path, that
width has to come from somewhere else.

Deriving it from the highest volume present in the mapping is wrong: a user who
maps volumes 1–9 gets `Vol. 01`, then adds volume 10 and every existing file is
now misnamed. The width must not move.

So the row stores `volume_count`, pre-filled from corpus `issue_count` when a
guess is available and user-editable otherwise, floored at two digits as D37
already specifies. It is metadata, not a path, so §6.11 permits it.

### 5.6 The corpus becomes lazy and optional

The corpus is only needed to *suggest*. A user whose bundle is fully mapped
should never download it.

- Fetch on first need — when a session has at least one unmapped file.
- A missing or undownloadable corpus degrades to "no guess", never to an error.
- `rusqlite` and `ring` remain compile-time dependencies, so
  `xcode-select --install` remains a documented prerequisite. Feature-gating
  them would fragment the binary and strip guesses from exactly the users who
  need them most.

---

## 6. Flow

### 6.1 Commands

```
comic-renamer map   <source> --dest <dir> --preset komga [--move|--copy] [-o plan.jsonl]
comic-renamer scan  <source> --dest <dir> --preset komga [--move|--copy] [-o plan.jsonl]
comic-renamer apply plan.jsonl
comic-renamer undo  plan.jsonl
```

`map` is the interactive session. `scan` stays exactly as specified in §6.13 —
non-interactive, scriptable, and the right command for an already-mapped
bundle. When `scan` finds unmapped files it reports the count and names `map`.

`map` takes `-o` and defaults to `comic-renamer-plan.jsonl` exactly as `scan`
does. It must not write to a temp directory: `undo` takes the *plan* path and
derives the manifest beside it (D38), so a plan that vanishes makes the run
un-undoable — losing the single guarantee §6.6 exists to preserve.

`apply` and `undo` are untouched.

### 6.2 Status model

`mapped` or `unmapped`. Nothing else. A mapped row has a series and, where
applicable, a volume; an unmapped row stays where it is and is reported.

The current `ReviewReason` variants do not survive as statuses. `Ambiguous`
carries `Vec<SeriesRecord>` and that payload is exactly a picker's candidate
list — the type was right and was being used to refuse rather than to offer.

### 6.3 The mapping session

Opens when `map` finds unmapped files. Leads with plain orientation text:

> Below is every file in your bundle we could not match: its filename, our
> guess, and a spot for you to approve or correct the final name.
>
> `thecompleteelfquest_vol1` → *The Complete ElfQuest Volume 1* → **The
> Complete ElfQuest Volume 1: The Original Quest TPB**

Then the table:

```
 Unmapped files in "Humble Comics Bundle: ElfQuest"          12 of 40 approved

 filename                      our guess                    final name
 ───────────────────────────────────────────────────────────────────────────────
 thecompleteelfquest_vol1      The Complete ElfQuest Vol…    The Complete ElfQuest Volume 1: The Original Quest TPB ▏
 thecompleteelfquest_vol2      The Complete ElfQuest Vol…    ✓ The Complete ElfQuest Volume 2
 elfquest_siege_vol1           ElfQuest: Siege at Blue M…    ✓ ElfQuest: Siege at Blue Mountain
 elfquest_kingsofthebrokenw…   (no guess)                    ▏

 ↑↓ row   ⏎ approve   e edit   ^A approve all remaining   ^S done
```

Every row needs explicit approval. `^A` exists because a 300-file bundle
cannot want 300 keystrokes; it approves the remaining guesses as-is and is
reported in the final confirmation.

### 6.4 Editing a row

The stored truth is structured fields. The user may instead type a sentence,
which is parsed into those fields with the result shown before it is accepted.

```
 thecompleteelfquest_vol1

 type a title, or edit the fields directly
 ┌────────────────────────────────────────────────────────────────────────┐
 │ The Complete ElfQuest Volume 1: The Original Quest TPB                 │
 └────────────────────────────────────────────────────────────────────────┘
 parsed as
   series   The Complete ElfQuest
   volume   1
   edition  The Original Quest TPB

 files to   The Complete ElfQuest/The Complete ElfQuest Vol. 01.cbz

 ⏎ accept   tab edit a field directly   esc cancel
```

**The destination line is the safety mechanism.** It renders through the real
preset, so a mis-parse is visible before anything is stored. Parsing at *edit*
time with the result on screen is categorically different from parsing at
rename time: a bad parse costs a correction, not a misfiled file.

The boundary detection already exists and is validated. `split_title` in
`xtask/src/lib.rs` locates `Volume N:` mid-title, roman numerals,
`Volume 1 (Second Edition)`, trailing `#7` and `(#1-6)`, measured against
2,223 real Humble titles.

**It is not the parser, though — it is a grader, and the distinction matters.**
It returns normalised word vectors, strips punctuation, and discards the volume
number it found, because comparison was all it ever needed. The parser needs
the user's string verbatim, the volume as an integer, and byte offsets. So the
boundary-detection logic moves to core and both callers share it; the grader is
not retrofitted into a parser.

### 6.5 Confirmation and the two offers

On `^S`: a filename → final name list, then

1. Rename now? Writes a plan and runs `apply` on it in this session.
2. Contribute these mappings? §7.

### 6.6 The TUI writes a plan; it never renames directly

`map` produces a plan and hands it to `apply`. This is not negotiable and is
the reason the rebuild is an excision rather than a rewrite: it keeps
invariant 2 (`apply` recomputes destinations from metadata, never trusting a
path string — D14), invariant 6 (no-clobber at the syscall), `TreeGuard`
proving refusals move nothing, the free-space reserve, the collision report,
and the plan-as-undo-record. The Task 13 disclosure *is* the final
confirmation screen's content.

---

## 7. Contribution

### 7.1 Mechanism

`gh pr create`. The user is already authenticated, no credentials pass through
this tool, and no HTTP client is added.

When `gh` is absent or unauthenticated: write the rows to a file, print the
path and the exact commands to run. Never a silent failure, never a degraded
half-submission.

### 7.2 What the PR contains

Only approved rows, and only the fields in §5.1. **No filesystem paths** —
the row is keyed by a normalised stem, so a contribution cannot leak directory
layout, usernames or library locations. The builder must confirm this holds
for the `bundle` provenance field too.

Rows the user corrected but chose not to contribute stay local.

### 7.3 Which characters are rejected, and which are sanitised

**Owner decision, 2026-10-22: the forbid-list splits.**

§6.11 rejects contributed data containing `:` among much else. That was correct
when a contributed rule was a small patch over trusted corpus titles. It is not
correct now, because the contributed `series` field *is* the title: **595 of
2,223 real Humble titles (26.8%) contain a rejected character** — 587 colons,
7 slashes, 4 `?`, 2 `"`. `Nemesis: Rogues' Gallery` and
`Hypnotwist / Scarlet By Starlight` would be unstorable as published.

**The principle, amended 2026-10-23.** An earlier draft of this table stated the
split as two character lists and put `/` in the reject column — while the
paragraph above it named `Hypnotwist / Scarlet By Starlight` as a title that
must be storable. That contradiction propagated into the implementation plan's
constraints and from there into a module doc and a task brief before it was
caught. The split is therefore stated as a rule first:

> **Reject what is invisible to a human reviewer. Sanitise what is merely
> hostile to a filesystem.**

A reviewer reading a pull request can see a colon or a slash and judge it. They
cannot see a zero-width space or a Cyrillic `е` — `Hell<U+200B>boy` renders to
the same folder as `Hellboy`, so accepting it creates a duplicate series that
survives review. That, not path-shapedness, is what divides the two columns.

| | |
|---|---|
| **Rejected** — invisible, or unrepresentable | NUL and all C0/C1 controls · tab, CR, LF · zero-width `U+200B–200F`, `U+FEFF`, `U+00AD` · bidi controls `U+202A–202E`, `U+2066–2069` · mixed-script confusables (at most one of Latin, Cyrillic, Greek) · non-NFC · a field that is empty, whitespace-only, or only dots · leading `=` `+` `-` `@` (spreadsheet formula injection) · per-field byte cap · the rendered-destination budget |
| **Stored verbatim, sanitised by the preset on the way to a path** | `:` `/` `\` `|` `"` `<` `>` `?` `*` · trailing dot or space · Windows device names |

`preset::komga::sanitize` already implements every entry in the right-hand
column, with 255-byte-per-component truncation that does not split a UTF-8
sequence. It is not to be duplicated in the validator.

Invariant 1 is unaffected: the preset remains the sole path constructor, and a
bad row can still only produce a wrong *name*. A field is never used as a path
component directly, so `../../x` is not an escape — it sanitises to a visible,
inert `.. - .. - x`.

Storing the real title verbatim also keeps the table reusable by a preset with
different path rules — Kavita and Plex are Phase 2 — and lets a contributed row
be checked against the distributor's published title, which a pre-sanitised
table cannot support.

### 7.4 §6.11 is now load-bearing

`CLAUDE.md` currently defers three §6.11 items — total path budget, rule-count
cap, file-size cap — on the grounds that contributed rule data is Phase 3. **A
contribution pipeline is Phase 3.** All three ship with this work, enforced in
CI on every PR *and* re-validated at `apply`, because the plan file is
untrusted too.

**Amended 2026-10-30, after a search found no such call.** `apply` does not
re-run `MappingRow::validate` or `rendered_budget_ok`. It protects itself a
different and stronger way: it recomputes every destination from the row's
metadata through the preset and never trusts a path string from the plan
(invariant 2, D14). Re-validating the *fields* would catch a forbidden
character in a series name; recomputing the *path* makes a tampered plan unable
to place a file anywhere the preset would not have put it anyway, which is the
threat the sentence above was written for. The field and 120-byte checks run
where a row enters the system — on load, in `map`, in `seed` — and the row and
file-size caps run in CI. `rendered_budget_ok` is called only by tests and
cannot fail for Komga, because `sanitize` truncates every component at 255
bytes; it is kept for Phase 2 presets with other path shapes.

So: enforced in CI and at every entry point, and the plan file is defended by
recomputation rather than re-validation. Said here rather than left as a known
gap, because an unexplained mismatch between a spec and its code invites
someone to "fix" the code.


§6.11 names the three without numbers. These are my call, and a reviewer should
push back on any of them:

| Cap | Value | Why |
|---|---|---|
| Per field | 120 bytes | Already in `overrides.rs`; unchanged. |
| Rendered destination | 255 bytes per component, 1024 total | "Total path budget" cannot mean a path — a row contains none (invariant 1). It means the destination the preset *renders* from the row: series, volume, subtitle, extension. CI cannot know `dest_root`, so it budgets the part the row controls and leaves headroom. |
| Rows per PR | 2,500 | Must clear the §10 seeding PR of 2,036 rows, or seeding bypasses the gate it exists to enforce. |
| Bytes per mapping file | 2 MiB | ~1,850 rows is well under 1 MiB at this row width. |

---

## 8. What happens to the gate

Task 14's three thresholds dissolve. They are not re-derived.

They existed because a wrong guess silently misfiled a file. Nothing is
stored without approval now, so there is no silent anything. A 10.12%
suggestion-error rate that fails today's gate 21x is unremarkable for a
pre-filled field a human confirms.

Replacing them:

| Check | Kind |
|---|---|
| No two mapping rows share a key with different fields | **hard CI gate** |
| Every §6.11 forbid-list rule, per contributed field | **hard CI gate** |
| Key normalisation unchanged against the ground-truth fixture | **hard CI gate** |
| Mapping coverage of `humble-ground-truth.tsv` | reported |
| Guess quality (`located`, currently 91.6%) | reported, non-gating |

`cargo xtask measure` keeps the reporting rows and loses the exit code.

---

## 9. Risks

- **The first user of every new bundle does all the work.** Inherent to the
  reframe and accepted: guesses plus one-step contribution are the mitigation,
  and the second user of that bundle pays nothing.
- **Key normalisation is a migration hazard**, now much reduced: the rule
  makes no claim about which parts of a filename mean anything. §5.3.
- **A fifth marketplace may name files in a way nothing here anticipates.**
  Minimal normalisation is what makes that a new mapping file rather than a
  migration.
- **Untrusted data now flows in.** §7.3.
- **`volume_count` is user-supplied and affects padding.** A wrong value gives
  a wrong *name*, never a wrong *location* — invariant 1 holds.
- **Guess quality is now a UX cost, not a correctness one.** A bad guess means
  typing, which is the right place for that cost to land.

---

## 10. Seeding the shipped table

**Owner decision, 2026-10-22: seed from the ground-truth fixture, reviewed
before it ships.**

Shipping empty would make goal 1 describe nobody at launch, and §1.2's
zero-ambiguity measurement a property of a table that does not exist. So the
2,036 Humble keys become the first `data/mappings/*.tsv`, alongside hand-mapped
files for Fanatical, Global Comix and Sweet Shop (§5.3.1) so that every
marketplace has coverage and a regression fixture from day one.

The fixture holds `bundle`, `title`, `stem` only. `volume`, `subtitle` and
`volume_count` have to be derived by running §6.4's parser over 2,223 titles —
and **shipping that output unreviewed would violate §4's "nothing stored
without a keystroke" anti-goal on the very first commit.** So seeding is a
two-step task: generate, then review in the TUI, then commit. The TUI is
therefore a dependency of seeding, not the reverse.

One fixture defect to handle: line 1267 has no `stem` and must be dropped or
repaired, which is why 2,224 lines yield 2,223 usable rows.

**Open, and a release blocker rather than a build blocker:** `data/LICENSE`
attributes rule data to GCD under CC BY-SA 4.0. These titles came from Humble's
order API, and D31 committed them as *test data* with no URLs, prices or
account data. Shipping them as the product's dataset in a public repository is
a different use and needs its own licence answer. Building and reviewing the
table locally does not wait on this; publishing it does.

---

## 11. Left to the builder

- Whether `map` on a fully-mapped bundle is an error, a no-op, or falls
  through to `scan` behaviour.
- TUI library. My call: `ratatui` with `crossterm`, version pinned at
  implementation time against this workspace's `rust-version`. (That was 1.85 when this was written and is 1.88
now: `instability`, which `ratatui` pulls in, needs 1.88, as do five other
dependencies.)
  Hand-rolling three scrolling columns with inline editing is a few hundred
  lines of redraw and cursor arithmetic that breaks on resize and multi-byte
  input.
- How shipped mappings are sharded across `data/mappings/*.tsv` — per bundle
  reads well in PRs; per publisher or per letter may scale better.
- Whether `scan` should refuse outright when nothing is mapped, or produce a
  plan covering only the mapped rows.
- Migration of the existing `data/overrides.toml` rows into the mapping table.
- Terminal capability floor, and the fallback when stdout is not a TTY.

## 12. Decision log entries this proposes

> **Folded into `2026-10-19-comic-renamer-design.md` §13 on 2026-10-23.**
> That copy is the decision log of record; this list is what was proposed.
> If they differ, §13 is right.

Next free number was **D39**.

- **D39** Titles are looked up, not derived. Measured: 0 key collisions over
  2,223 verified rows against 10.12% silent false positives from the resolver.
- **D40** Key normalisation is minimal — NFC, lowercase, strip
  non-alphanumerics — asserting 2,036 keys; a change is a breaking data
  migration. Stripping marketplace noise was tried and withdrawn: it bought
  190 rows of 2,036 (9.3%) and was already wrong for Global Comix's hyphenated
  ISBN and Sweet Shop's type suffixes. Deciding which parts of a filename are
  noise is inference, which §1.3 removes.
- **D41** `bundle` is a lookup tiebreaker, not part of the key, so a future
  cross-distributor collision is a new row rather than a migration against
  D40's freeze.
- **D42** Mapping storage is TSV, two layers, local winning; user data in
  Application Support, never Caches.
- **D43** The row carries `subtitle`, not an invented `edition`. Series and
  volume alone put 280 of 2,223 titles (12.6%) on a shared destination.
- **D44** `volume_count` is stored per row, because D37's padding width cannot
  be derived from a growing table without renaming files already placed.
- **D45** The mapping session writes a plan and delegates to `apply`; it never
  touches the filesystem itself, and the plan is not written to a temp
  directory because D38 makes it the undo anchor.
- **D46** A user may type a sentence; the parse is shown with its rendered
  destination before acceptance.
- **D47** §6.11's forbid-list splits by character class: the set that can
  influence a location is rejected, the set that is merely filesystem-hostile
  is stored verbatim and sanitised by the preset. 26.8% of real titles depend
  on this.
- **D48** Contribution is `gh pr create`; §6.11's three deferred caps ship
  with it, at the values in §7.4.
- **D49** The corpus is lazy and optional; absent corpus degrades to "no
  guess", never to an error.
- **D50** Task 14's thresholds dissolve rather than being re-derived, because
  nothing is stored without approval.
- **D51** The shipped table is seeded from the ground-truth fixture, generated
  by the parser and reviewed in the TUI before commit. Licence for publishing
  Humble-derived titles is open and gates release, not development.
- **D52** One mapping row serves every format of a comic. `.cbz`, `.pdf` and
  `.epub` share a stem, and `PlanRow` already carries `extension` separately.
- **D53** The guess source dispatches on marketplace. The corpus is one
  strategy; for Fanatical, title-casing the stem beats it, because the
  filename is already the title modulo sanitisation.
