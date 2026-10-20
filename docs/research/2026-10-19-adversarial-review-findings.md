# Adversarial review findings — design doc 2026-10-19

Three independent cold reviews of `docs/specs/2026-10-19-comic-renamer-design.md`.
None had access to the conversation that produced it. Verdicts:

| Review | Brief | Verdict |
|---|---|---|
| **Builder check** | Plan from this doc with no access to the author. What would you have to guess? | 7 blocking ambiguities |
| **Premise adversary** | Argue against the project. Attack prior art and the central claim. | **Reshape, before Phase 0.** Two-month single-engineer project after reshape. |
| **Engineering pre-mortem** | Assume it ships and goes wrong. Enumerate how. | **Reject as written.** 41 findings, several demonstrated by destroying test files. |

Neither adversary says kill the idea. Both say this document is not buildable as written.

---

## 1. The reshape — the finding that supersedes the architecture

**The design's central claim is wrong.** §1.3 argues that recovering a title from
a squashed filename requires world knowledge, therefore that knowledge must be
hand-curated, therefore the curated list is the product. The missing step is that
**this knowledge is already published and downloadable.**

**Mechanism:** squash-normalise both sides — strip every non-alphanumeric from the
filename *and* from each corpus title — then longest-prefix match. All ten §7
fixtures resolve, including the five the document calls unreachable. The
apostrophe in `Bourdain's` needs no reconstruction because the matched record
already contains it.

**What this deletes:** word segmentation, the stoplist, and the title-casing
rules. Three of §12's six builder items, plus open question 11.4, become moot.

**What survives as curated knowledge — and only this.** Verified first-hand
against the `gcd-django` models, which GCD designates as schema ground truth:

- `gcd_series` has no parent-series field, no "collects" field, no
  collected-edition flag.
- `gcd_series_publication_type` ships three rows: `book`, `magazine`, `album`.
  An omnibus is `book`; so is an original graphic novel; so is a prose novel. The
  schema cannot express *that something is a collection*.
- `gcd_series_bond` looks like the relation and is not — all six bond types are
  numbering continuity (`..._numbering_continues`, `merge`), rendered "X
  continues at Y". None means "collects".
- `format`, `binding`, `publishing_format` are uncontrolled `CharField(255)`,
  hand-typed, no vocabulary. A weak hint, never a key.
- The only path is transitive and unusable: collected issue → stories →
  `Reprint` → original stories → original series. Depends on GCD's most
  incomplete layer, both story ends are nullable, an omnibus spanning titles
  yields N parents needing a majority vote, and a missing link is
  indistinguishable from no relationship.
- Comic Vine and Metron model no such relation either. ComicTagger's maintainer,
  on exactly this: *"Comic Vine is not completely consistent on when they are a
  series or separate when it comes to trade paperbacks."*

> A corpus can tell you the title is *The Complete Elfquest*.
> Nothing can tell you it belongs in `Elfquest/`.

**But that layer is ~100× smaller than the document assumes**, because it keys
per collected-edition **line**, not per volume. `Hellboy Omnibus → Hellboy` is
one row covering 5+ books. `The Complete Elfquest → Elfquest` covers 9.
`Usagi Yojimbo Saga → Usagi Yojimbo Saga` is an identity row encoding "Saga is
not strippable" with no stoplist at all. Low hundreds for practical coverage, and
stable — a new omnibus volume needs no new row. This is a defensible moat; a
per-title list was not.

**Highest-leverage change:** default the series folder to the published title,
make strip-to-parent an opt-in preset flag. The one thing no database can supply
then stops gating Phase 1, and Phase 1 ships with zero curated data.

---

## 2. The week-one kill test

Settles the reshape empirically before anyone writes a resolver.

1. Pull Metron. Squash-normalise the 40 real filenames from the reference bundle.
   Count unique-hit coverage.
2. Separately, hand-write the line→parent rows those 40 files need. Count them.

**High coverage + a short map confirms the reshape. Low coverage vindicates the
original design.** Days, not a phase.

---

## 3. Critical — irreversible harm

**3.1 The disk figure was wrong by 6×, and it blocked Phase 1.** Reference bundle
is 31.2 GiB / 40 files, not 5.1 GB. `du`'s partially-materialised on-disk reading
was recorded as the logical total; the placeholder count generalised from a
four-file sample. Development machine has 11.0 GiB free on a 98%-full volume, so
copy-by-default could not complete on the project's own corpus. *Resolved: §13
D1, move-by-default.*

**3.2 Case and Unicode collisions silently destroy files.** Demonstrated: `Hellboy/`
and `hellboy/` fold to one directory and the second write overwrites the first
with no error. NFC `Pokémon` and NFD `Poke\u0301mon` do the same, and `readdir`
then returns the NFC form so the tool cannot observe what it did. §6.2 and §11.3
treat collisions as exact string equality — these rows are *not* equal as
strings, so they pass detection, both execute, and one file is gone.
Unrecoverable under `--move`. **Mirror failure:** on a Linux ext4 Komga server
the same two variants become two separate series, so identical rule data
produces different library structure on macOS and on the server.
*Requirement: compare on an NFC-normalised, case-folded key; probe the
destination filesystem at scan time; NFC output as a hard invariant.*

**3.3 On a Dropbox source, a move is a server-side delete.** The reference bundle
lives in Dropbox and move is now the default. *Requirement: refuse `--move` on a
cloud-sync source root without explicit override.*

**3.4 "Rules are data, not code, therefore safe by construction" is false.** A
destination path string **is** an arbitrary-write primitive. `path.join(dest,
"../../..")` escapes. `startsWith(dest + sep)` containment passes while a
symlinked subdirectory writes outside the tree. And the attack that defeats human
review: `C:\...` and `\\server\share` are inert strings on a macOS reviewer's
machine and absolute paths on a contributor's Windows box — **review happens on
macOS, exploitation on Windows.**

*Replacement architecture:* rules carry **metadata fields only** — series,
volume, format class, edition class, subtitle — from a closed vocabulary. Rules
never contain paths or separators. The preset, which is first-party code, is the
**sole path constructor**. A bad rule can then produce a wrong name, never a
wrong location.

*Schema must forbid, per field:* separators `/` `\`; `.` and `..`; leading `.`;
drive-letter and UNC prefixes; leading `/`; NUL and all C0/C1 controls; tab, CR,
LF; leading `=` `+` `-` `@` (spreadsheet formula injection — filenames are
attacker-controlled); bidi controls U+202A–202E and U+2066–2069; zero-width
U+200B–200F, U+FEFF, U+00AD; non-NFC input; mixed-script confusables (a Cyrillic
`е` creates a duplicate series invisible in a diff); trailing dot or space (legal
on APFS, silently stripped on Windows, creating a fresh collision); Windows device
names including with extension — `CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`,
`CON.pdf`, all of which create fine on APFS; `< > : " | ? *` (`:` renders as `/`
in Finder); per-field byte cap ~120 plus a total path budget; rule-count and
file-size cap per PR; duplicate keys within or across rule files with a defined
merge order. Enforce in CI on every PR **and** re-validate at `apply`, because
the plan file is untrusted too. Containment via `realpath` per component or
`O_NOFOLLOW` — never a string prefix.

**3.5 Tier 1 `ready` auto-applies with no rule id, no corpus version pin, and no
report path.** A wrong curated rule is the confident-wrongness anti-goal at
supply-chain scale, with no mechanism for a user to discover or report it.

---

## 4. Execution safety requirements

Move-by-default helps materially: a same-volume move is `rename()` — atomic, zero
extra space, no check needed. It also removes the "originals never touched" net.
Cross-volume move is copy + unlink at full cost.

`scan`, in order: `realpath` both roots and **refuse if either is inside the
other**; `stat().dev` on both to record same-volume; record destination case and
normalisation sensitivity; sum **logical** sizes via `stat().size`, never `du`;
flag dataless sources (`st_blocks == 0 && st_size > 0`); select and record a
per-row mechanism (`rename` / `hardlink` / `clone` / `stream`) and the required
free space, which is zero for the first three.

`apply`, before touching anything: re-stat sources against the plan and refuse
changed rows; re-check space for stream rows only, refusing the whole run when
required exceeds available minus `max(1 GiB, 10%)`; re-verify the mechanism and
**never silently fall back to a 31 GiB stream**; refuse move on a cloud source
root without override. Per row: temp → `fsync` → verify size and hash → `rename`
→ then unlink. Journaled for `--resume` and `undo`. Destinations opened `O_EXCL`.

**Instrument warnings.** `du` and `stat -f %b` cannot see APFS clone savings —
3,084 MiB reported for a ~2 GiB clone; only a `df` delta reveals it. Node's
`COPYFILE_FICLONE` silently falls back to a full copy and `COPYFILE_FICLONE_FORCE`
returns `ENOSYS` on same-volume APFS, so **Node can neither force nor verify a
clone.** Hardlink and rename are the only genuinely free mechanisms available.

**TOCTOU is live, not theoretical.** The reference bundle's file count read 42,
then 40, during the review — it is actively syncing. The source set mutates
between scan and apply.

---

## 5. Packaging — the §6.6 mechanism does not work

Tested by building, not by reading documentation:

- **Node 24.13.1 SEA is CommonJS-only.** `mainFormat: "module"` is silently
  accepted at build time and dies at runtime with `SyntaxError: Cannot use import
  statement outside a module`. **Ink and Clack — §6.6's entire justification —
  are pure ESM.**
- An unsigned post-`postject` binary on arm64 is **SIGKILL, exit 137, no
  output.** A non-technical user sees nothing happen at all.
- Artifact **112.5 MiB**. `ExperimentalWarning` every run. Native addons cannot
  load from SEA.
- **What works:** bundle Ink down to CJS with esbuild on 24.13.1 — lowest risk,
  available today. Or pin whichever newer Node first ships working
  `mainFormat: "module"` plus `--build-sea`, **verified by executing a build**,
  since the docs page was wrong for 24.13.1. Bun `--compile` would remove the
  problem class entirely (native TS + ESM) but is not installed here and is
  untested — worth a spike. `pkg` is deprecated; do not plan on it.
- **A notarized `.pkg` or `.app` is mandatory, not polish.** You cannot staple a
  notarization ticket to a bare Mach-O, so an unstapled binary fails Gatekeeper
  offline.
- **§6.6's rejection of Rust is self-defeating:** Tauri's backend *is* Rust. The
  stated reason for rejecting Rust — that the core would not be reusable from the
  Mac app — inverts. TypeScript may still be correct; the argument given for it
  is not.

**Recurring cost, previously unpriced:** Apple Developer $99/yr with notarization
included. Windows Authenticode ~$200–400/yr for OV, hardware-token key required
since 2023, and OV does **not** silence SmartScreen until reputation accrues; EV,
which does, is ~$300–700/yr. **Roughly $300–500/yr to ship signed on both
platforms.** Verify at purchase.

---

## 6. Naming taxonomy — the fixture suite is unrepresentative

- **§6.2 contradicts §7.** "The Complete" is stripped from the folder but kept in
  the filename, while "secondedition" is dropped from both. That requires **three
  word classes**; the document names two.
- **Fixtures miss the dominant corpus shapes.** No composed edition + subtitle
  (`grendelomnibusvolume1_hunterrosesecondedition`). No **word addition**, where
  the published title contains a word the filename omits (`maskomnibus…` → *The
  Mask Omnibus*) — which breaks naive squash-prefix matching and needs
  article-insensitive comparison. **20 of 40 bundle files need bespoke underscore
  handling.**
- **Rules keyed on bare filenames collide across publishers.** `vol1.pdf` and
  `001.pdf` recur; a content discriminator is required.
- **ASCII `'` versus U+2019** splits one series into two sibling folders.
  Normalise quotes as an output invariant.
- **Windows-illegal names enter the corpus in Phase 3 but Windows ships in Phase
  5** — colons arrive in bulk, too late to fix the data cheaply.
- **One EPUB in the corpus, zero non-PDF fixtures**, while scope claims PDF, CBZ
  and EPUB.
- MAX_PATH is a medium risk, not high — fixtures run 64–112 of 260 characters,
  biting only with a deep destination root plus a long title.

---

## 7. The plan file and the two review surfaces

- **The review threshold is keyed on review count, not row count.** A 500-row run
  with 20 reviews auto-applies **480 unseen rows**. Key it on total rows.
- **TSV is an unsafe contract.** Formula injection, Excel codepage and CRLF
  corruption, tabs inside titles, no row identity, no TOCTOU guard — and `apply`
  trusts a file edited by hand or by an LLM.
- **Plan files leak.** Absolute source paths, and potentially a Comic Vine API key
  via unredacted error URLs, into AI assistants and public PRs. The document
  explicitly blesses handing the plan to an assistant.
- **The walk and the plan file share no contract.** "Skip" has no file
  representation. Two divergent code paths in the safety-critical layer.
- **Destination inside source** re-ingests the tool's own output on a recursive
  scan.
- The builder check's independent framing: the plan file has no defined
  eligibility rule — does `apply` refuse every row still marked `needs review`,
  or execute everything not marked `failed`? An assistant that mass-flips
  statuses satisfies the letter of §6.4 and defeats its purpose.

---

## 8. Prior art and motivation

- **ComicTagger already ships `--split-words`, default on**, plus template
  renaming and a batch CLI, and its tracker logged the `mazebook` case in 2021.
  Scored 71/113 on this bundle class. **The genuine remaining gap is narrower
  than claimed: PDF collected editions, renamed without opening the file.** State
  this honestly in the README rather than claiming new ground.
- **Komga auto-locks metadata fields edited by hand**, so the 40-file motivating
  case is roughly twenty minutes of clicking in Komga's own UI with no 31 GiB
  copy. **The tool's real justification is several hundred files, or portability
  across multiple apps** — not the case in the spec.
- **Kavita files every file with no issue number as a "Special."** The entire
  corpus has no issue numbers. Phase 1 can therefore succeed into an app that
  will not shelve the output correctly, which makes PDF→CBZ load-bearing rather
  than backlog and puts the Kavita preset in question.
- **§6.3 pre-registers Panels as probably a no-op**, then ships it in the launch
  set and names it in Phase 2's exit criterion.
- **Phase 0's done-criterion names three apps; its deliverable names thirteen.**

---

## 9. Data source terms

| Source | Bundle derived corpus? | Notes |
|---|---|---|
| **Metron** | **Yes** | CC BY-SA 4.0. Commercial use explicit. ShareAlike covers the data, **not the software source**. Attribution: `Comic data from Metron, licensed under CC BY-SA 4.0`. API 20/min, 5,000/day, **each user of a multi-user app needs their own account**. |
| **GCD** | Probably | ~212,300 series. Pages conflict: download page CC BY-SA 4.0, Data Distribution wiki CC BY 3.0. The 2021 Archive.org dump carries BY 3.0 alongside an open "upgrade to CC BY-SA 4.0" item, so the wiki is stale. Resolve by email. |
| **Comic Vine** | **No** | Non-commercial only; "do not edit, manipulate or reproduce on any other medium"; link-back required; "don't build a competing product"; revocable at discretion. 200/resource/hour plus an undocumented 420 throttle. **GPL-3.0 cannot relicense their data.** Live opt-in lookup only. |

**Consequence for sequencing:** every provider is bring-your-own-key for live
lookup, so **no API can fix first-run coverage for a stranger with no
credentials.** Only a bundled corpus can. That argues for corpus-building in
Phase 1 rather than lookup at Phase 4.

Cover images are excluded from all three licenses. Irrelevant — we want titles.

---

## 10. Builder-check ambiguities not already covered above

1. What does a tier-1 rule key on, and what is the acceptance bar for a file with
   no rule? Needs a generalisation target as a number against held-out files, not
   the ten fixtures, which ten hardcoded rows would pass while proving nothing.
2. Where do renamed files land? The command surface has no destination argument
   and every fixture path is relative. The realistic case is merging into a
   library that already contains `Hellboy/` — a far more common collision than
   two source rows colliding with each other.
3. Is bulk corpus seeding in scope, and what review rate is acceptable for v1?
4. What does the source tree actually look like? Every fixture is a flat `.pdf`;
   real bundles ship PDF + CBZ + EPUB per title in nested folders alongside
   non-comic extras. Recursion and multi-format duplicates are Phase 1
   architecture, and the fixture suite hides both.
5. May the shipped binary fetch the corpus over the network, and is that on by
   default? "Versions independently of the binary" implies a runtime fetch, which
   turns Phase 3 from a data file into a distribution channel with a trust model.
6. If the spike kills Panels, does a third app substitute or does Phase 2 pass on
   two?

---

## 11. Checked and dismissed

- NUL bytes: Node's `fs` already rejects them (`ERR_INVALID_ARG_VALUE`).
- `~` expansion: no expansion in `path`/`fs`; live only if `cp -c` gets spawned.

---

## Provenance

Findings 3.2, 3.4, and section 5 were demonstrated empirically — test files
destroyed and SEA binaries built — in the session scratchpad. Neither
`/Users/mike/sites/comic-renamer` nor the reference bundle was modified; verified
by `git status` and bundle mtime inspection.
