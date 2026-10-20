# Comic Renamer — Design

**Status:** draft — **§1.3, §6.1, §6.2, §6.6 and the §8 phase sequence are
superseded pending the kill test.** See
`docs/research/2026-10-19-adversarial-review-findings.md` before planning
against any of them.
**Date:** 2026-10-19
**Owners:** Mike (engineering), Rob (UX)
**Org:** Business Casual

---

## 1. Intent

People who buy comics in bulk — Humble Bundle, publisher sales, DRM-free
storefronts — receive files whose names are hostile to every comic library
application in existence. `hellboyomnibusvolume1_seedofdestruction.pdf` is a
real filename from a real bundle. Dropped into Komga or Kavita, it produces a
series called "hellboyomnibusvolume1 seedofdestruction" and no volume number.

Comic Renamer points at a folder of those files and proposes a correct,
app-specific destination path for each one. The user reviews every proposal,
fixes what's wrong, and only then does anything touch disk.

The tool is a renamer. It is not a library manager, a reader, a downloader, or
a metadata service.

### Why this is harder than it looks

The obvious framing — "parse messy filenames with regular expressions" — is
wrong, and we have evidence. A real bundle
(`HumbleDarkHorseAnniversaryBundle`, 40 files, 5.1 GB) was examined before
this document was written:

1. **There are no issue numbers.** Every file is a collected edition:
   omnibuses, `vol1`, archives, one-shots. The original spec's naming model
   (`{series} - #{issue}`) does not describe a single file in the bundle.

2. **There are no word boundaries.** Filenames are squashed lowercase:
   `plantsvszombieszomnibusvolume3`, `xomnibusvolume1`, `mazebook`. Separator
   characters, where they exist, are inconsistent — the underscore in
   `hellboyomnibusvolume1_seedofdestruction` marks a subtitle, while the
   underscores in `nemesis_rogues_gallery` are just spaces.

3. **Splitting squashed text requires world knowledge, not an algorithm.**
   `mazebook` is correctly one word (Jeff Lemire's *Mazebook*). `xomnibus` is
   two. A dictionary-based word splitter gets both wrong. No amount of
   cleverness in the parser fixes this, because the information isn't in the
   string.

4. **File contents are not a usable fallback.** The bundle is **31.2 GiB
   across 40 files**, the largest single file 2.54 GiB. PDF has no
   `ComicInfo.xml` equivalent, and the one file inspected carried an empty PDF
   title with an author field reading `Adobe InDesign CC 2014 (Macintosh)` —
   the name of the layout tool, not the book. Parsing 31 GiB to recover nothing
   is a bad trade.

   *Correction.* An earlier draft of this document said 5.1 GB and "39 of 40
   were cloud placeholders." Both figures were wrong: 5.1 GB was `du`'s
   partially-materialised on-disk reading mistaken for a logical total, and the
   placeholder count was generalised from a four-file sample. The files are now
   fully local. The filename-only conclusion survives, but it rests on format
   and parse cost, not on placeholder state.

**Therefore: the filename is the only input, and recovering a real title from
it is a knowledge problem, not a parsing problem.** That single conclusion
drives the entire architecture below.

---

## 2. Goals

- Turn a folder of badly-named comic files into correctly-named files laid out
  the way the user's chosen app expects.
- Never modify or move anything without explicit confirmation of a reviewed
  plan.
- Be honest, per file, about how the proposed name was derived and how much it
  should be trusted.
- Support several target applications, because each one wants a different
  layout, and make adding the next one cheap.
- Get better over time by accumulating knowledge contributed by its users.

## 3. Non-goals

Explicitly out of scope. Some are permanently out, some are backlogged with
open questions (section 11).

- Reading comics.
- Browsing or managing a library.
- Converting file formats (PDF to CBZ is backlogged, not planned).
- Opening, rasterising, or inspecting the inside of any file.
- Watching folders in the background.
- Accounts, cloud sync, paid tiers.
- Guaranteeing the destination app can actually *read* the files it renames.
  Renaming is the product; format compatibility is the user's problem.

## 4. Anti-goals

Outcomes that would make this a failure even if it technically worked.

- **Silent data loss.** A user losing comics to a bad rule is unrecoverable
  reputational damage for a file-mutating tool from an unknown publisher.
- **Confident wrongness.** A plausible-looking wrong name that a user approves
  without noticing is worse than an obvious failure. Low-confidence guesses
  must look low-confidence.
- **A parser nobody can extend.** If adding a rule requires understanding a
  regex engine, outside contributions stop and the knowledge base dies.
- **Requiring technical skill to run.** If the first release needs a Python
  install, a package manager, or a PATH edit, the intended audience never gets
  past step one.

---

## 5. Constraints

| Constraint | Source | Consequence |
|---|---|---|
| First release is a terminal program | Owner decision | Must be genuinely polished, not a bare script. Assume low technical skill. |
| Must later power a Mac app designed by Rob | Owner decision | Core logic must be a reusable library with no CLI assumptions, consumable from a GUI shell. |
| Install must be one step | Derived from audience | Single self-contained executable. No runtime prerequisite on the user's machine. |
| Filename-only, never open files | Evidence, section 1 | No PDF/archive parsing anywhere in the pipeline. |
| Contributions arrive as GitHub pull requests | Owner decision | Rule data must be plain, diffable, human-reviewable, and **not executable**. Every PR is untrusted input from the internet. |
| Repository is GPL-3.0 | Pre-existing in repo | See open question 11.1. |

---

## 6. Architecture

Three parts: a **resolver** that turns a filename into structured metadata, a
**preset** that turns metadata into a destination path, and a **plan** that the
user reviews before anything executes.

```
  filename  ──▶  RESOLVER  ──▶  metadata  ──▶  PRESET  ──▶  destination path
                    │                              │
                    │                              └── Komga / Kavita / Panels
                    └── tier 1  curated rules      (one preset per app)
                        tier 2  ComicVine (opt-in)
                        tier 3  algorithmic guess
```

### 6.1 The resolution ladder

Three sources of truth, tried in descending order of trust. The tier that
answers becomes the file's status and is shown to the user.

**Tier 1 — curated rules (shipped with the tool).**
A plain data file mapping known filenames and known titles to their correct
metadata. This is where `mazebook → Mazebook` lives, and
`anthonybourdainshungryghosts → Anthony Bourdain's Hungry Ghosts`, and the fact
that `X` is a series while `Omnibus` is a format. **This is the product's
actual value**, not a fallback for edge cases. Result status: `ready`.

**Tier 2 — ComicVine (optional, user supplies their own API key).**
If configured, unresolved filenames are looked up for an educated guess. The
tool must be fully functional with no key — the only difference is how many
rows need review. Result status: `needs review`, annotated with the match and
its score. This reverses a non-goal in the original spec, deliberately.

**Tier 3 — algorithmic guess (always available).**
Word segmentation, volume/edition token extraction, title casing. Always runs,
always lowest trust, always reviewed. Result status: `needs review`, or
`failed to parse` when it cannot produce anything coherent.

Nothing above `ready` is ever executed without the user editing or explicitly
accepting it.

### 6.2 Naming rules

Decided by the owner against a ten-file fixture set (section 7).

| Rule | Decision |
|---|---|
| Volume word | Always abbreviated: `Vol. 1`. Normalised regardless of input form, so `volume1` and `vol1` both produce `Vol. 1`. |
| Volume number | Not zero-padded. `Vol. 1`, not `Vol. 01`. |
| Series folder name | Format and edition words are **stripped** to find the series. `Hellboy Omnibus Vol. 1` lives in `Hellboy/`; `The Complete Elfquest Vol. 1` lives in `Elfquest/`. |
| Underscore meaning | Ambiguous by nature — resolved per title by the curated list, never by a global rule. Unresolved titles get a subtitle guess and a review flag. |
| Subtitle separator | ` - ` in the filename. |
| Real-title oddities | Preserved. `Zomnibus` stays `Zomnibus`, because that is the published title. |
| Edition qualifiers | `secondedition` and similar are dropped from output. |
| Small-word casing | Standard title case — `of`, `the`, `and`, `vs.` stay lowercase mid-title. Produces `Plants vs. Zombies`. *(Author's call, flagged for owner review.)* |

**The stoplist is knowledge, not syntax.** "The Complete" and "Omnibus" are
stripped; "Saga" is not, because *Usagi Yojimbo Saga* is the real series name.
Nothing in the strings distinguishes them. The stoplist therefore lives in
curated data alongside the title rules, and is expected to grow by
contribution.

**Edition-stripping creates collisions by design.** Two printings of the same
volume can resolve to the same path. This is a collision to be surfaced and
resolved in the plan, not a parse failure. See open question 11.3.

### 6.3 Presets

A preset owns both the folder structure and the filename format for one target
application. Launch set: **Komga, Kavita, Panels**. Presets are selected by the
user per run.

Presets are authored from the research spike's output (section 9), not from
guesswork. One known risk: Komga and Kavita are folder-convention
applications where the path genuinely carries the metadata, but **Panels may
rely on embedded `ComicInfo.xml` instead**. If the spike finds Panels is
metadata-driven, renaming for Panels may be close to a no-op, and the preset
should be reconsidered before it is built.

### 6.4 The plan

The scan produces a plan — one row per file — and never touches the
destination. Each row carries: source path, current filename, proposed series,
proposed filename, full destination path, status, resolving tier, and a reason
when the status is not `ready`.

Two review surfaces, selected by volume:

- **Small review count (threshold around 25):** an interactive terminal walk,
  one file at a time, accept / skip / edit.
- **Large review count:** the plan is written to a file. The user fixes it in
  their own editor — or hands it to an AI assistant, which is explicitly an
  expected workflow — and then applies it. Bulk editing beats any terminal UI
  at several hundred rows, the file is diffable and resumable, and it is
  already the right shape for a contribution PR.

### 6.4.1 Execution modes

Execution **moves** by default, with `--copy` and `--hardlink` as flags. The
reference bundle is 31.2 GiB and the development machine has 11.0 GiB free on a
98%-full volume, so copy-by-default is not merely expensive — it cannot
complete, and Phase 1's own exit criterion would be unreachable.

A same-volume move is a rename: the bytes never travel. That eliminates the
entire class of interrupted-copy failures, including the truncated
multi-gigabyte PDF that is indistinguishable from a complete one. A
cross-volume move is a copy followed by a delete and **must** verify size and
checksum before it unlinks anything.

| Mode | Extra disk | Originals | Constraint |
|---|---|---|---|
| `--move` (default) | none, same volume | gone from source | cross-volume degrades to copy + verified delete |
| `--copy` | equal to payload | kept | needs free space ≥ payload |
| `--hardlink` | none | kept | same volume only; cloud-sync clients mishandle links |

**The mode must be disclosed before execution, with the real numbers.** Not a
flag buried in `--help` — a pre-flight statement that names the mode, its
consequence, and which alternatives are actually possible on this machine:

```
  MOVE MODE — files will be relocated, not copied.

  412 files · 31.2 GiB · same volume → instant, no extra disk used
  Originals will no longer exist at their current paths.

  --copy       keeps originals, needs 31.2 GiB free (you have 11.0)
  --hardlink   keeps originals, no extra disk, same volume only

  Type MOVE to continue:
```

Typed confirmation is required on first run and whenever the plan is
cross-volume. Free space is checked before any row executes, and the run
refuses rather than partially completes.

### 6.5 Contribution flow

When a user corrects a file the tool got wrong, that correction is exactly the
data tier 1 needs. The tool offers to submit it back as a GitHub pull request.

This forces two hard requirements:

1. **Rule data is data, not code.** No regular expressions, no templates, no
   executable anything in a contributed rule. Every PR is untrusted input from
   a stranger, reviewed by a human, and must be safe by construction.
2. **The rule corpus versions independently of the binary.** A contributor
   whose fix is merged should not wait for a new signed Mac build to see it
   land.

### 6.6 Implementation language

**TypeScript on Node, distributed as a single self-contained executable.**

| Candidate | Verdict |
|---|---|
| **TypeScript / Node** | **Chosen.** Compiles to a single executable with no runtime prerequisite. Best-in-class terminal UI tooling (Ink, Clack) for the "polished" bar. Critically, the same core module is consumable by a Tauri or Electron Mac app, so Rob designs the GUI in his native medium and there is exactly one parser. Matches the owner's existing stack (TS, Zod, Vitest); Zod is the right tool for validating untrusted rule PRs. |
| Python | Rejected. Distribution to non-technical users is the weak point, and the Mac app path is bad — either rewrite the core in Swift or embed a Python runtime in the app bundle. |
| Bash | Rejected. No structured data, no testability, no terminal UI worth the word "polished", no path to a GUI. |
| Go / Rust | Viable and better at single-binary distribution, but hand the Mac app the same problem as Python: the core is not reusable from a web-technology GUI without an FFI layer or a second implementation. |

**Toolchain note:** the active Node on the development machine is 18.15.0,
which is end-of-life. Node 24.13.1 is installed. The project should pin Node
24 — single-executable support in particular is not viable on 18.

### 6.7 Command surface

```
comic-renamer scan <source> --preset komga -o plan.tsv
comic-renamer apply plan.tsv [--move]
```

---

## 7. Fixture suite

Ten files chosen because each breaks a different rule, with owner-approved
destinations. **This is the acceptance test for the resolver.** Every one of
these must pass before any phase is called done.

| Source filename | Destination path |
|---|---|
| `mazebook.pdf` | `Mazebook/Mazebook.pdf` |
| `xomnibusvolume1.pdf` | `X/X Omnibus Vol. 1.pdf` |
| `hellboyomnibusvolume1_seedofdestruction.pdf` | `Hellboy/Hellboy Omnibus Vol. 1 - Seed of Destruction.pdf` |
| `plantsvszombieszomnibusvolume3.pdf` | `Plants vs. Zombies/Plants vs. Zombies Zomnibus Vol. 3.pdf` |
| `47ronin.pdf` | `47 Ronin/47 Ronin.pdf` |
| `nemesis_rogues_gallery.pdf` | `Nemesis Rogues Gallery/Nemesis Rogues Gallery.pdf` |
| `usagiyojimbosagavolume1secondedition.pdf` | `Usagi Yojimbo Saga/Usagi Yojimbo Saga Vol. 1.pdf` |
| `anthonybourdainshungryghosts.pdf` | `Anthony Bourdain's Hungry Ghosts/Anthony Bourdain's Hungry Ghosts.pdf` |
| `cyberpunk2077omnibusvolume1.pdf` | `Cyberpunk 2077/Cyberpunk 2077 Omnibus Vol. 1.pdf` |
| `thecompleteelfquest_vol1.pdf` | `Elfquest/The Complete Elfquest Vol. 1.pdf` |

Five of the ten are unreachable without curated knowledge: `Mazebook` being one
word, the apostrophe in `Bourdain's`, `X` being the series, "The Complete"
being strippable, and "Saga" *not* being strippable. The remaining 35 files in
the source bundle are available as a second-tier corpus.

---

## 8. Phases

| Phase | Deliverable | Done when |
|---|---|---|
| **0 — Research spike** | Convention writeups for all 13 candidate apps (section 9), in `docs/research/`. | Komga, Kavita and Panels are documented well enough to author presets without guessing. |
| **1 — Core + CLI** | Resolver (tiers 1 and 3), Komga preset, `scan`/`apply`, plan-file review, copy-by-default execution. | All ten fixtures pass. The tool is usable end to end on the real bundle. |
| **2 — Breadth + polish** | Kavita and Panels presets. Interactive review walk and the volume threshold. | The preset abstraction survives three real apps without being rewritten. |
| **3 — Knowledge corpus** | Curated rule corpus at scale, schema validation, independent versioning, PR emission from a corrected plan. | An outside contributor can land a rule fix without touching code. |
| **4 — ComicVine tier** | Optional key, lookup, match scoring, graceful absence. | Review counts on a fresh bundle drop measurably. Tool still works with no key. |
| **5 — Distribution** | Signed and notarised macOS binary; Windows and Linux builds; install docs written for a non-technical reader. | Rob installs and runs it from the README with no help. |
| **6 — Mac app** | GUI shell over the unchanged core, designed by Rob. | Feature parity with the CLI for scan, review, apply. |

**Backlog, with open questions attached:** PDF-to-CBZ conversion (what is the
lift? how are double-page spreads and resolution handled?); Mylar, Calibre and
Ampchor presets (Ampchor's rules exist and the owner can supply them, but it is
in private beta); folder watching.

---

## 9. What the research spike must answer

For each application, per owner instruction, researched deeply:

**Launch:** Komga, Kavita, Panels.
**Also in scope:** Mylar, Calibre, Ubooquity, YACReader, ComicRack, Chunky,
Paperback, Suwayomi, Plex/Kometa, Ampchor.

Per app:

1. Is the convention officially documented, or reverse-engineered from
   community practice? Cite sources.
2. Is it **filename-driven**, **folder-driven**, or **metadata-driven**
   (`ComicInfo.xml`)? If metadata-driven, is renaming meaningful at all?
3. Exact folder structure expected, including series, volume, and any
   publisher or language level.
4. Exact filename grammar: how volume, issue, year, and edition are written
   and parsed.
5. Collected editions specifically — omnibuses, trades, archives. How are they
   distinguished from single issues? This is the actual shape of our data and
   is frequently undocumented.
6. Which characters are forbidden or problematic, and how the app sorts.
7. Supported file formats, and behaviour when it meets one it cannot read.
8. Preset effort estimate, and any blockers.

Known in advance: **Ampchor publishes no naming convention** — it performs its
own metadata matching, is in private beta, and supports CBZ/CB7/CBT but not
CBR. **Calibre** manages its own directory layout on import and may not be
renameable-toward at all.

---

## 10. Repository conventions

```
docs/specs/     design documents    (this file)
docs/plans/     implementation plans
docs/research/  spike output
```

Dated `YYYY-MM-DD-<topic>.md`. Business Casual is expected to produce several
projects; this layout is deliberately generic and tool-agnostic so a stranger
reading the public repo needs no context to navigate it. *(Author's call.)*

---

## 11. Open questions for the owners

**11.1 — GPL-3.0 and the contribution model.** The repo already carries
GPL-3.0. Two consequences worth a conscious decision. First, once outside
contributors land code under it, their copyright cannot be relicensed without
their permission or a contributor licence agreement — so any future closed or
paid version of the core is foreclosed unless that is set up now. Second,
GPL-3.0 is incompatible with Mac App Store distribution; direct download is
unaffected. Keep GPL-3.0, switch to a permissive licence, or keep it and add a
CLA?

**11.2 — The repository is private.** The spec's trust posture is explicitly
open-source, and the contribution flow requires a public repo. When does it
flip?

**11.3 — Collision policy.** When edition-stripping sends two files to the
same destination path, what should happen: refuse the row, auto-suffix, or
require the user to disambiguate in the plan?

**11.4 — Small-word casing.** Section 6.2 proposes standard title case,
which renders `Plants vs. Zombies` rather than the `Plants Vs Zombies` in the
owner's original answer. Confirm or override.

---

## 12. Left to the builder

- Word-segmentation algorithm for tier 3, and how its confidence is scored.
- Plan file format (TSV is assumed above but not mandated) and its schema.
- Rule-data file format and its Zod schema.
- How the rule corpus is versioned and delivered independently of the binary.
- Terminal UI library choice and the visual design of both review surfaces.
- Single-executable build mechanism.
- Test strategy beyond the ten fixtures.

---

## 13. Decision log

Decisions taken after the initial draft, with the reasoning that produced them.
The sections above have been updated; this log exists so the *why* is not lost.

**D1 — Execution defaults to move.** Reverses the draft's copy-by-default.
Forced by measurement: 31.2 GiB payload against 11.0 GiB free. Same-volume move
is also safer than copy against interrupted-run corruption. `--copy` and
`--hardlink` remain available, and the active mode is disclosed with real
numbers before execution. See §6.4.1.

**D2 — Case and Unicode collision detection is a Phase 1 blocker, not open
question 11.3.** Demonstrated empirically on this project's own APFS volume:
`Hellboy/` and `hellboy/` collapse into a single directory and the second write
silently overwrites the first, with no error. NFC and NFD spellings of the same
title collide the same way. Under copy-by-default this costs a duplicate; under
move-by-default it destroys the only remaining copy. No comparison of path
strings can detect either case. Detection must exist before `apply` ships.

**D3 — Licensing: MIT for code, CC BY-SA 4.0 for the corpus.** Replaces the
repo's inherited GPL-3.0 and an earlier CC0 recommendation for the corpus.
GPL-3.0 forecloses Mac App Store distribution and blocks any future relicensing
once outside contributors hold copyright. CC0 is unavailable for the corpus
because every viable data source is share-alike.

Source terms, each verified against the provider's own published statements:

| Source | Bundle a derived corpus? | Terms |
|---|---|---|
| **Metron** | **Yes** — the clean source | CC BY-SA 4.0. Commercial use explicitly permitted. ShareAlike **applies only to the data, not to the software source**, per Metron's own guidance. Attribution: `Comic data from Metron, licensed under CC BY-SA 4.0`, linked, noting any changes. Per-record credit not required. |
| **GCD** | Probably, unresolved | Larger (~212,300 series) but its own pages conflict: the download page states CC BY-SA 4.0, the Data Distribution wiki states CC BY 3.0. Confirmed from both directions. Resolve by email before building on it. |
| **Comic Vine** | **No** | API terms are non-commercial only, forbid republishing the data in another form, and permit key revocation at their discretion. Live opt-in lookup only — never bundled, never load-bearing. |

Cover images are excluded from every one of these licenses. Irrelevant here: we
want titles, not art, and staying away from images avoids the question entirely.

ShareAlike not reaching the source code is the finding that makes this
workable. MIT code and a CC BY-SA 4.0 corpus coexist in separate directories
with separate `LICENSE` files and a two-line README note. *Pending owner
confirmation; see open question 11.1.*

**D3-A — Live lookup is bring-your-own-key for every provider, and the shipped
corpus is what makes a first run usable.** Metron permits bundling a derived
corpus but limits API accounts to 20 requests/minute and 5,000/day, and states
that **each user of a multi-user application needs their own account**. Comic
Vine is BYOK by its terms and caps at 200 requests per resource per hour. So no
live API can be the answer to first-run coverage for a stranger with no
credentials — only the bundled corpus can. This strengthens the case for
building the corpus in Phase 1 rather than deferring lookup to Phase 4.

**D4 — Resolved against the document: the resolver architecture in §6.1 is
wrong.** Three cold reviews agree. Squash-normalising both the filename and a
corpus of published titles, then longest-prefix matching, resolves all ten
fixtures — including the five §7 calls unreachable — with no word segmentation
and no stoplist. The curated layer survives only for title → series folder,
which is absent from the GCD, Metron and Comic Vine schemas alike, and it keys
per collected-edition **line** rather than per volume, making it roughly 100×
smaller and stable against new releases.

Consequences: §1.3's inference from "knowledge problem" to "hand curation" is a
non sequitur. §6.2's stoplist and casing rules are deleted, as is open question
11.4. Three of §12's six builder items vanish. §6.6's rejection of Rust is
self-defeating because Tauri's backend is Rust, and its single-executable
mechanism does not work on the Node version it pins.

**Not yet rewritten, deliberately.** The reshape is gated on the kill test
(findings §2): pull Metron, squash-normalise the 40 real bundle filenames, count
unique-hit coverage, and separately count the line→parent rows those files need.
High coverage plus a short map confirms the reshape; low coverage vindicates the
original design. Rewriting §6.1 before that result would be guessing twice.

**D5 — Deferred to the findings document.** 41 engineering findings and 12
premise findings, including four classes of irreversible data loss, the
replacement for the false "rules are data, therefore safe" safety argument, and
a previously unpriced $300–500/yr code-signing cost. They are recorded in
`docs/research/2026-10-19-adversarial-review-findings.md` rather than inlined
here, because most of them attach to sections this document will rewrite.

**D6 — Launch roster stays Komga, Kavita and Panels, contingent on the spike.**
Chosen over narrowing to Komga alone. *Consequence requiring action:* Phase 8's
exit criterion for Phase 2 — "the preset abstraction survives three real apps" —
cannot stand as written, because the spike may find Kavita files the entire
corpus as Specials and Panels is metadata-driven. Reword to: *survives every app
the spike validates, minimum two.* The spike must settle both before any preset
is authored.

**D7 — Distribution is Homebrew, unsigned, for release one.** $0/yr against
$300–500/yr for signed binaries on both platforms, and a brew install is exempt
from the Gatekeeper failure that makes an unsigned downloaded binary die as a
silent SIGKILL. *Large consequence:* **this may remove the single-executable
requirement entirely.** A Homebrew formula can depend on `node` and install the
package directly, so the whole SEA problem class — CommonJS-only builds, bundling
Ink down from ESM, the 112.5 MiB artifact, notarization, stapling — does not
arise for release one. The "one-step install" constraint in §5 is still satisfied,
just by Homebrew managing the runtime rather than by embedding it. Revisit if and
when a double-clickable download is needed; a notarized `.pkg` and the $99/yr
Apple Developer account become mandatory at that point, not optional.

**D8 — Repository goes public at Phase 3**, when the contribution flow requires
it. Private through Phases 1 and 2. *Consequence:* the contribution flywheel
cannot turn before Phase 3, which reinforces D3-A — the bundled corpus, not
community contributions and not a live API, is what has to make a first run
usable. Scrubbing absolute home-directory paths from the findings document is
deferred to the same milestone.

**D9 — Licensing confirmed: MIT for code, CC BY-SA 4.0 for the corpus.** The
root `LICENSE` has been replaced; the inherited GPL-3.0 is gone. Copyright holder
recorded as "Business Casual" — adjust if a different legal entity or named
individuals are preferred. Outstanding when a corpus directory first exists:
add `data/LICENSE` (CC BY-SA 4.0) and the README attribution line, `Comic data
from Metron, licensed under CC BY-SA 4.0`, linked to source and license, noting
that the data has been adapted.
