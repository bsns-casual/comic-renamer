# The Humble table, the review, and licensing

Moved out of `CLAUDE.md` on 2026-10-30. The short version that stayed there:
the shipped table is 12 rows in `data/decisions.tsv`; the 2,031-row Humble
table is decided but deliberately not in this repository or its history; both
licences are settled and must not be reopened.

`review/README.md` is the live document for the table itself — what it was,
where it still exists, and the recipe if it is ever to ship. This file is the
reasoning behind that decision, not a substitute for it.

---

## What the generator produces, and how its output was classified


**The Humble table is generated and deliberately not shipped.** `cargo xtask seed`
also writes about 2,200 candidate rows to `target/seed/humble.tsv`, which is
untracked. It waits for the maintainer to review it, because D51 and the rebuild's
§4 forbid storing rows nobody approved, and committing 2,200 unreviewed rows on
the first commit would do exactly that. On its last run (2026-10-25) the
generator built 2,223 rows, rejected 1 (a short line), found 0 key collisions, and
de-duplicated to 2,204. Its own classification of the 2,223, after the issue
column (D59–D61) and the casing rule (D62) landed on 2026-10-26 and 2026-10-27:

```
1525   68.6%  clean
 189    8.5%  issue written in the verified title (#N)
 128    5.8%  corpus settled the series casing          (D62, not reviewed)
 117    5.3%  issue copied from a sibling of the same title
 111    5.0%  corpus confirms the series (colon kept, volume recovered)
  59    2.7%  issue taken from the filename (D61)
  54    2.4%  series casing guessed, corpus silent      (D62, reviewed)
  21    0.9%  bare trailing number in the series (not a volume marker)
  18    0.8%  volume marker swallowed into the subtitle (colon came first)
   1    0.0%  multiple colons
```

**"Clean" is an upper bound.** It means no heuristic fired, not that a human
agreed.

**The bare-trailing-number class fell from 197 to 21**, which is the issue
column's whole point: 59 rows had an issue number the filename vouched for,
and 117 more are other files of those same titles. Before that propagation
(D61 as amended) one comic landed in two Komga series folders —
`Hacktivist/Hacktivist 0001.cbz` beside `Hacktivist 1/Hacktivist 1.cbz` — and
nothing could report it, because the paths differ and so do the folder
collision keys.

**A derived issue is not a confirmed one.** `scripts/review-rows.py` keeps
both derived classes in the review pile. With D62's guessed class beside them
the page puts **266 rows** in front of a human, every one arriving with a
better proposal than before. One of the 59 is wrong in a way only a reader catches:
`"Shrine of the Monkey God!" in Kramers Ergot 9` is one story inside the ninth
book of a numbered anthology, and the rule makes the quoted story the series.
Routing derived rows out of that pile would have hidden it.

**The colon class was 129 rows and is now 18, because `seed` asks the corpus.**
Where a verified title ends in a volume marker and its stem carries a colon,
the generator looks the stem up: `Attack on Titan: Before the Fall` is one of
seven Attack on Titan series GCD records, with 17 issues, so the colon belongs
to the name and `Vol. 9` is the volume — not an arc of `Attack on Titan` with
the marker stranded in a subtitle. 111 of 127 such titles confirm;
`docs/research/2026-10-25-the-corpus-can-confirm-a-colon.md` measures it and
names the 16 that do not. Three things that lookup deliberately does not do,
each of which cost something to find:

- **It never consults `issue_count`.** Four `Battle Angel Alita: Last Order`
  rows hold volumes 16–19 of a run GCD records at 15. The corpus confirms the
  *series*, never the volume, which is independent support for D44.
- **It compares squashed names on both sides.** GCD's own `name_key` strips
  stopwords (`attackontitanbeforefall`, no "the"), and matching against that
  alone reported this very series as unconfirmed: 68 instead of 111.
- **It never touches the 12 shipped marketplace rows.** Those were confirmed
  title by title, and a keystroke outranks a lookup (D50, D51). `data/decisions.tsv`
  is byte-identical after a regeneration, which is the check that proves it.

A missing corpus is "no guess", not an error (D49): `seed` prints one line and
falls back to the parser for every row. `measure` still requires one.

**Tests:** `cargo test --workspace` runs 616 and ignores 9 (counted 2026-10-30
by `awk`-summing the `test result:` lines, which is the number to quote: 616
passed, 0 failed, 9 ignored; 615 before `scan` read the local layer, 613 before D-a and D-d, 612 before the review of the one-decisions-table
change, 610 before the change itself, 606
before the two fix commits of 2026-10-29 and 2026-10-30, 596 before the casing
rule, 537 before
the issue column, 534 before
the corpus-confirms-a-colon change, 530 before
D58, 510 before a four-seat adversarial review, 500 before the whole-branch
review's fix wave, 409 and 7 before session-scoped undo), with no compiler
warnings. **The 595 this file carried until 2026-10-27 was one short** — it was
summed by hand; the baseline measured mechanically on the same tree is 596.
**The whole suite is 625, and the nine ignored have not been run since
2026-10-26**, when the five volume-gated ones passed — the casing rule touches
none of their code. See "What the volume-gated suite does and does not
protect" above before assuming a change needs them. The nine `#[ignore]`d
tests need real hardware or the real corpus release, and **CI runs all nine**:
five are volume-gated (`COMIC_RENAMER_TEST_VOLUME`; four in `apply_exec.rs`,
one in `cli.rs`) and four are corpus tests (`COMIC_RENAMER_TEST_ZIP`, in
`corpus.rs`). The workflow sweeps `--ignored` over each of those three
binaries wholesale (`verify.yml` lines 67, 71 and 115), so no workflow change is
owed for the split. The seven-test claim was false until the
rebuild's Task 13: the workflow ran the three volume-gated tests, and its own
comment said the four corpus tests were not run. Task 13 added the `rust-corpus`
macOS job, which downloads the pinned release, checks its sha256 **and**
`sizeBytes`, and runs those four. The five volume tests run in the `rust` job
on a RAM disk, because a same-volume move is a rename and the streamed path, the
`placed` state and abort-on-failure are otherwise unreachable. This is read
from the workflow file. The four corpus tests were run locally against the pinned
release at the end of the rebuild (green, 54 s).

**Observed on GitHub at last**, when the rebuild merged to `main` on 2026-10-24:
run 40061195019, all six jobs green, including the first ever execution of
`rust-corpus` ("corpus tests against the real release") and `mappings`
("mapping caps (rows per PR, file size)"). Both had existed only on an unpushed
branch until then. **The green `mappings` job in that run did not check the row
count** — see the PR-only caveat above; it was a push to `main`.

- `COMIC_RENAMER_TEST_ZIP=<gcd-*.zip>` — four tests against the real corpus
  release: install-and-open, coverage rates, and the publisher hint's effect
  on real titles. Run by the `rust-corpus` job.
- `COMIC_RENAMER_TEST_VOLUME=<mount on another device>` — five tests that need
  a genuine second volume. A 1.3 GiB HFS+ RAM disk works:
  `hdiutil attach -nomount ram://2726297` then
  `diskutil eraseVolume "HFS+" CRTEST <dev>`. **Under ~1.1 GiB every run is
  refused by the free-space reserve**, which is how the reserve is specified.


---

## The review, the casing rule, and the two data-safety findings

**0. The review is decided, and the table is deliberately not in this
repository.** 2,204 of 2,204 decided, exported, validated through the real
gates — and then excised from HEAD and from all 220 commits on 2026-10-30,
because the owner decided this repository should not carry Humble-derived
titles. That is a decision, not a blocker. **`review/README.md` is the live
document**: what the table was, the two places it still exists, and the
recipe if it is ever to ship.

**Nothing here is waiting on a licence.** Both licences are settled and
enforced — see item 1 below. What stopped the table shipping was the owner's
call about redistributing a marketplace's order history, which is that
marketplace's terms rather than a licence question, and `data/LICENSE` says so
in its own closing paragraph.

**The artifact's store is no longer the only copy.** If the review is reopened,
re-dump the collection and re-run `scripts/export-decisions.py`; the export is
sorted by key and bundle, so a regeneration is a readable diff.

**-1. The casing rule is merged, pushed and green.** Three commits went to
`main` on 2026-10-27 — `bf4ac06` (hide skipped), `a8d8b47` (the all-caps class
and confirm-what-is-shown) and `5e5b494` (D62, the corpus casing). **CI run
40454635024 is green on all six jobs**, `rust-corpus` in 1m32s among them, so
the one branch that cannot be unit-tested here has been executed against the
real release. Locally: 606 passing, 9 ignored, `check-mappings` green,
`check-review-page.cjs` green, no compiler warnings, the shipped table
byte-identical after a regeneration. The docs commit `9d097f6` that followed
was still queued when it was last looked at; it changes no code.

The job most worth reading is `rust-corpus`, for the opposite reason to last
session: D62's corpus branches cannot be unit-tested here, because the cache
holds `gcd.sqlite` and not the zip the job downloads. `casing_outcome` is
split out from the lookup precisely so the *decision* is testable without a
corpus — but `corpus_casing`, which reads `Corpus::lookup`, is exercised only
by `cargo xtask seed` against the local cache.

**CI run 40283202212 for the issue column came back green on all six jobs**,
including `rust-corpus` in 1m47s, which was the open question at the previous
close. The only annotations are advisory: `actions/checkout@v4` targets the
deprecated Node 20 and is being forced onto Node 24, and `ubuntu-latest`
migrates to Ubuntu 26 from 2026-10-19. Neither fails anything yet.

**The issue column before it** is 19 commits, `747b080..84a0439`, pushed
2026-10-26. `cargo xtask seed` leaves the shipped table byte-identical except
the four rows that legitimately improved (`Hammer Head`, `Minotaur`,
`DIE: Loaded`, `Glarien: Origins` each carried `#1` as subtitle text).

**D62 leaves one gap, and it is the owner's to close.** The Humble path has no
equivalent of the fifth column in `fixtures/marketplace-ground-truth.tsv`, so a
ruling made on the review page does not reach the generator. Four titles are
already in that position — the owner typed `BLAME!`, `DIE`, `CURSE WORDS` and
`ODY-C`, and the corpus proposes `Blame!`, `Die`, `Curse Words`, `Ody-C`. The
stored decisions win today, because the page fills a decided row from the
store and the new proposal reaches it only through `↺`. A later `DIE` volume
in a new bundle would arrive proposing `Die` again.

**The "two likely typos" this file used to record here are not typos.** Checked
2026-10-30: neither `Exquiste Corpses` nor `Front Knife` exists anywhere in
`review/`, `data/` or `fixtures/`. The committed export has `Exquisite Corpses`
(rows 714–715) and `First Knife` (row 766), both spelled correctly, against
Humble's own `EXQUISITE CORPSES, VOL. 1` and `FIRST KNIFE` — so they are D62
casing decisions behaving correctly. The note described the artifact store
before the export, or it was never right. Do not go looking for them.

The licence gate was checked before pushing and is **not** crossed: the
shipped table carries only Fanatical, Global Comix and Sweet Shop bundles,
`target/seed/humble.tsv` is gitignored, and no Humble row reached
`data/mappings/`. Individual Humble titles appear in tests and docs as before,
which D31 already sanctions as test data.

One thing to carry forward about the review that produced those commits:
**four seats found nine real defects and three of them were in my own new
tests.** A test that passes for the wrong reason is the recurring shape here —
`!refused.is_empty()` satisfied by an unrelated refusal, a non-digit typed on
an empty field where the parse fails either way, a helper that did not assert
it had broken what it claimed to break. Run the mutation, do not reason about
it.

**0. Two data-safety findings, closed, merged and green.**
`docs/research/2026-10-23-two-pre-existing-data-safety-findings.md` describes
both. The first was a data-loss path: a hand-written manifest beside a forged
plan made `undo` delete a library file no run placed. The second needed no
adversary: a second terminal running `scan` substituted a plan under a
confirmation already given. Session-scoped undo addresses them with invariant 11
(D54, D58) and by reading the plan once and executing from that copy. Merged to
`main` on 2026-10-25 at `aad24f6`, and **CI run 40163809503 is green on all six
jobs** — the first time every job has run on this work.

One note on how it got there, because the shape recurs: the merge went out with
the volume job red. `cargo test --workspace` reported 534 then and **skips the
nine `#[ignore]`d tests**, so "534 passed" was never the whole suite, and one of those
nine asserted the pre-D58 truth that a reversed copy leaves the library empty.
Four visible tests were updated when D58 landed; the invisible one was not,
because that round's brief did not require the RAM-disk run. **Quote the gated
total, or run the volume suite** — 606 of 615 as of 2026-10-27, and the nine
ignored last ran on 2026-10-26.

The first fix then needed a second one. An adversarial seat showed D54's own
gate was check-then-act on the source and lost the file outright when raced —
`1 restored · 0 refused`, exit 0, a 400 MB payload in neither place, on the
first attempt. D58 replaced the unlink with a rename into the aside directory,
which is why invariant 11 now reads the way it does. The race is reproduced
deterministically in
`nothing_is_lost_when_the_source_is_truncated_inside_the_gate`
(`crates/core/tests/undo_authority.rs`): it triggers off the *destination's*
atime, which nothing before the gate touches, so the writer fires inside the
window by construction rather than on a timed guess. The
design's §7 left one question open that was deliberately not planned: sweeping
records written by an install that predates the sweep. No release has shipped,
so none exists.


---

## Licensing, and the decision not to publish the table


Then, one thing that gates a release, and one that is settled and recorded
here only so it is not reopened.

**1. Licensing is settled. Do not reopen it.** Both halves are written down and
one is enforced by CI:

- **Code: GPL-3.0-or-later** (`842a5d6`), in `LICENSE` and `Cargo.toml`. The
  `licence` job checks the licence title *and its version line*, `Cargo.toml`,
  and the Homebrew formula `release.yml` emits — and asserts MIT appears in
  none of them. The formula said MIT for a day after the switch because an
  earlier check read only the first line of `LICENSE`.
- **Data: CC BY-SA 4.0, attributed to the Grand Comics Database**, in
  `data/LICENSE`, covering `data/decisions.tsv` and `data/overrides.toml`.

That satisfies the owner's direction of 2026-10-30, "whatever suits our needs
most while staying open source." **This entry claimed the code was MIT until
2026-10-30**, nine commits after it became GPL and with a CI job asserting the
opposite three directories away. Two findings are kept below because they are
the reasoning, not an open question:

- **GCD-derived fields are already mixed into the Humble rows**, so the share-
  alike question is not separable. 128 rows had their series casing settled by
  the corpus (D62) and 111 had a colon confirmed by it — 239 of 2,223 rows carry
  a field the corpus decided. `data/LICENSE` ties rule data to GCD under
  CC BY-SA 4.0, and BY-SA propagates. So **keeping all of `data/` under
  CC BY-SA 4.0 with attribution to GCD is the answer that needs no new
  decision**; a looser licence for the Humble rows is not available without
  stripping the corpus's contribution back out. This is what `data/LICENSE`
  now says, and why it declines to split the table by provenance per field.
- **A licence cannot grant rights nobody has**, which is why the Humble
  question was never a licence question: it is whether redistributing a
  2,199-row compilation pulled from Humble's order API is permitted at all.
  That is Humble's terms of service, not copyright — individual titles are
  short factual strings and US law gives thin protection to factual
  compilations, while the EU has a separate database right. Nobody here is
  qualified to settle it, and **the owner did not: he removed the table
  instead**, which answers the practical question without anyone having to.

**The repository went public on 2026-10-30, and the Humble table was then
excised from history at the owner's instruction.** Sequence, because the order
is the point:

1. Public at 05:15, with the decided table in HEAD and in history.
2. Four minutes later, 0 forks, 0 watchers, 0 stars, and clone traffic of
   327/2 uniques — which is CI checkouts plus this machine, not third parties.
3. Made **private** for the rewrite, because a force-push on a public repo
   leaves the old objects fetchable by SHA until GitHub garbage-collects, so
   the window would otherwise have stayed open.
4. `git filter-branch` over all 220 commits removing
   `review/humble-decisions.tsv` and its sidecar, force-pushed, re-tagged,
   public again.

**Every commit SHA changed.** CLAUDE.md's twelve SHA references were
mechanically translated by matching commit subjects old-to-new (220 of 220
mapped unambiguously) and each one verified to resolve on `main`. **Commit
messages were not and cannot be** — a message citing another commit by SHA now
cites a commit that does not exist, and fixing that would require a second
rewrite that invalidated the SHAs again. Treat any SHA inside a commit message
dated 2026-10-30 or earlier as pre-rewrite.

What this does and does not achieve: the table is out of this repository and
out of its history. It was publicly readable for roughly four minutes with no
evidence of outside access, and `fixtures/humble-ground-truth.tsv` **still
holds 2,223 Humble titles** and is staying — `cargo xtask measure` and three
test binaries read it, and D31 sanctions filenames and titles as test data.
So "no Humble data is public" was never the outcome on offer; "the decided
2,031-row table is not published" is.

My earlier recommendation of "public without the Humble table" was made without
checking history and was not executable as stated. Recorded so nobody repeats
it.

**2. The Humble table review is complete: 2,204 of 2,204 decided**, counted
from the artifact's store on 2026-10-27 with 0 orphans, 0 malformed rows and 5
marked not-a-comic. 266 of them went through the review pile one at a time; the
remaining 1,784 were approved as proposed in one batch, at the owner's
instruction, after the corpus had settled the casing (D62).

**It is no longer in the repo**, and not in its history either — removed
2026-10-30, see the entry above. It was 2,031 rows in the shipped shape (2,199
before the D64 transformations), 680 series, 205 KB. `review/README.md` records
what it was, where it can be regenerated from, and the two transformations a
regenerated export still needs. `scripts/export-decisions.py`
made it from a dump of the artifact's `decisions` collection; it judges nothing
and never recomputes a key, because the frozen key rule has one implementation
and `review-rows.py` already had to delete a Python copy of it.

**It was validated through the real gates**, on 2026-10-27, by copying it to
`data/mappings/humble.tsv` and adding it to `SHIPPED_FILES`: 606 passed, 0
failed, covering `MappingRow::validate` over every row, `find_key_collisions`
over the joined table, and the frozen key pin. Both changes were reverted.

**That validation predates D63 and so does the recipe it implied. Do not
follow it.** `data/mappings/` is gone and gitignored, and **`SHIPPED_FILES`
does not exist in any code** — one `const SHIPPED` inside `shipped_mappings()`
replaced it, so there is no array and no size to bump.

**The two data transformations are done, 2026-10-30.** The file is 2,031 rows
in the shipped seven-column shape: `bundle` dropped, 168 repeated keys
collapsed, **zero disagreements across all seven fields** — D64's load-bearing
measurement, confirmed on the real decided table. Shipping it is now
`tail -n +2 review/humble-decisions.tsv >> data/decisions.tsv` and a `git rm`.
Checked by doing exactly that and reverting: the joined 2,043-row table gives
613 passed, 0 failed, at 132 KB against a 4 MiB cap, with zero key overlap
against the 12 rows already shipped.
**`review/README.md` is the accurate copy of this** — an earlier version of
this file said it "still describes the old shape", which was false and taught
agents to distrust the one correct document.

**`review/` is deliberately not `data/`.** A file under `data/` is
`include_str!`-embedded, which is an act of shipping, and
`the_decisions_file_is_the_one_that_ships` fails on arrival to say so — **at
the top level only.** A `.tsv` in a *subdirectory* of `data/` is seen by
neither that test nor `cargo xtask check-mappings`: an adversarial seat planted
this very 205 KB file at `data/mappings/humble.tsv` on 2026-10-30 and both
gates stayed green, with a positive control at `data/stray.tsv` failing
correctly. **Both holes are closed now**: `.gitignore` refuses to track such
a file, and `the_decisions_file_is_the_one_that_ships` walks `data/` at any
depth through `tsvs_under`, asserting exactly one table and that it sits at
the top rather than in a subdirectory. Read the test before assuming this
paragraph is still describing a gap.

Three things the finished review established, each measured rather than assumed:

- **Every key agrees with itself.** 0 keys whose rows disagree on a field,
  which is exactly what `find_key_collisions` refuses, so the table would
  survive a contribution as it stands.
- **No series is spelled two ways.** Eight were, across 58 rows, and all eight
  were normalised to the corpus's spelling on 2026-10-27 — including a bare
  `Attack on Titan:` with a trailing colon on 20 rows, and four spellings of
  `B.P.R.D. Hell on Earth`. Each pair would have produced two Komga folders.
  The genuine sub-series keep their colons, as the corpus records them.
- **Seven rows were approved against the generator and with the owner.**
  `East Of West` and `W0rldtr33` were proposed, but the owner had already typed
  `East of West` and `w0rldtr33` for other rows of the same series, so his
  spelling was applied to the rest. A keystroke outranks a lookup (D50, D51),
  and that holds for the owner's own earlier keystroke too.

**`map` is not the tool for it, and the owner chose a page instead.** `map`'s
`Entry` holds a filename and the resolver's candidates and never reads
`fixtures/humble-ground-truth.tsv`, so a `map` session would ask for 2,204
titles to be re-decided from 48.9%-accurate guesses while discarding the
verified titles `seed` already built the rows from. It is also 38 sessions,
one per bundle, because a contribution carries exactly one bundle name.

The review runs on **`scripts/review-page.html`**, published privately as an
artifact. Each card shows the filename, its bundle, the verified Humble title,
three prefilled fields and the Komga path they produce. Decisions live in the
artifact's own store, one document per 100 rows, and `scripts/review-rows.py`
builds the page's data from `cargo xtask seed`'s output. To regenerate and
republish, run `cargo xtask seed`, then `review-rows.py`, then publish
`scripts/review-page.html` **with the existing artifact's URL** — a different
file path would make a new artifact and orphan the decisions.

Three things that page learned the hard way, all fixed:

- **Writes merge, never replace.** It rebuilt each 100-row block from its own
  mirror and wrote it whole, so one failed read at load plus one Confirm
  destroyed the other 99 decisions.
- **A decision's identity is checked, not assumed.** The store is keyed on a
  row's ordinal in `seed`'s output; each decision also carries its row's key
  and bundle, and a mismatch is reported as an orphan rather than painted over
  the wrong file.
- **`scripts/check-review-page.cjs`** pins its path renderer against
  `komga.rs` with 13 cases copied from `preset_komga.rs`. The page mirrors the
  preset in JavaScript, which is not an invariant 1 violation — it constructs
  no path the tool acts on — but an unpinned mirror that drifts shows the
  wrong path to someone making 2,204 decisions.

Two controls changed on 2026-10-27, both from using it:

- **Hide skipped is its own checkbox.** "Hide confirmed" used to hide skipped
  rows too, and only on the per-row paint: `sweepHidden` tested
  `classList.contains("done")` and a skipped row carries `skipped`, so the row
  vanished on a repaint and came back on the next sweep — which `renderMore`
  runs for every batch. Both paths share one `hidden(node)` predicate now.
- **The scan section's bulk confirm acts on the rendered slice**, not the whole
  stack, so one press can no longer confirm two thousand rows nobody has
  scrolled past. `Show more` disarms it, because growing the shown set
  invalidates the count the press was agreed against, and the button stays
  after a batch instead of hiding.

**Invariant 7's bundle half matters here**: 158 of the table's 2,036 keys
appear under more than one bundle, so this is the table where a literal bundle
comparison would have started costing titles.

Still measured and **deliberately not implemented**: a whole-title corpus
lookup confirms 12 of the bare trailing numbers (`RoboCop 2`,
`B.P.R.D.: 1948`). It changes no field, only the report, and that class is 21
rows now rather than 197, so it would change nothing about the review's shape.

Then, in rough priority:

**Kavita is not a Komga-with-a-different-template.** The research in
`docs/research/2026-10-23-kavita-from-source.md` found Kavita is
filename-driven where Komga is folder-driven, so the Komga layout fragments a
Kavita library: **890 of 2,223 Humble rows (40%) would become one-book series**,
because Kavita's last series pattern is a catch-all and a filename with no volume
marker becomes its own series name. It also withdrew §8's stated reason for
PDF→CBZ conversion ("Kavita files every issue-number-less file as a Special");
that is true only for Book libraries. The backlog entries in the design's §8 and
§9 now say so. Conversion may still be worth doing, but not for that reason, and
writing PDF metadata is the more direct fix for both problems. **All of this is
read from source and was not confirmed against a running Kavita**, which the
research flags itself.

