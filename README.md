# Comic Renamer

Rename comic files bought in bundles into the layout your library app expects.

Point it at a folder, pick your app, and it identifies each file and shows you
every proposed rename. Nothing moves until you approve the plan. Files it cannot
identify are flagged for review rather than guessed at.

**Status: scan, apply and undo work. Komga is the only preset, and there is
no installer yet** — build from source. See [`docs/specs/`](docs/specs/) for the
design and [`docs/research/`](docs/research/) for the measurements behind it.

## Use it

```sh
cargo build --release
alias comic-renamer=./target/release/comic-renamer

# Identify everything and write a plan. Touches nothing.
comic-renamer scan ~/Bundles/humble-dark-horse --dest ~/Comics

# Read the plan, fix what needs fixing, delete rows you do not want.
$EDITOR comic-renamer-plan.jsonl

# Move the files. Discloses the cost, then asks you to type MOVE.
comic-renamer apply comic-renamer-plan.jsonl

# Changed your mind.
comic-renamer undo comic-renamer-plan.jsonl
```

The first `scan` downloads the corpus — a 34 MiB archive, hash-verified against
[`corpus/manifest.json`](corpus/manifest.json) — and caches it in
`~/Library/Caches/comic-renamer`, where it unpacks to 97 MiB.

Between one and two rows in three need a human, so reading the plan is the
normal path and not an escape hatch. Rows still marked `review` are left where
they are; `apply` never guesses.

Editing the plan means editing the **metadata** — the series, the volume, the
subtitle. It does not mean editing paths: the destination is recomputed from
metadata every time, so a path typed into a plan file is ignored rather than
obeyed. That is deliberate, and it is what makes a plan safe to hand to an AI
assistant or attach to a pull request.

| Flag | On | Does |
|---|---|---|
| `--dest <dir>` | `scan` | Where the library goes. Required. |
| `--mode copy` | `scan` | Keeps the originals. Clones on APFS, so it usually costs nothing. |
| `-o <file>` | `scan` | Where to write the plan. |
| `--lang <code>` | `scan` | Which language's editions to match. Defaults to `en`, and never falls back to another. |
| `--komga-url <url>` | `apply` | Refuses unless your Komga can survive a rename. Token from `COMIC_RENAMER_KOMGA_TOKEN`. |
| `--yes` | `apply` | Skips the typed confirmation. The disclosure still prints. |
| `--allow-cloud-move` | `apply` | Permits a move out of a cloud-sync folder, where it is a server-side delete. |

**Moving is the default, and it is destructive by design** — a same-volume move
is a `rename`, which costs nothing and cannot truncate a multi-gigabyte PDF
halfway. Every run writes an undo manifest beside the plan as it goes, so an
interrupted run is still reversible.

### Before you point it at a Komga library

A rename is a delete-plus-add to Komga, which rematches by size and then by
hash. **Hashing has to already be on** — switching it on afterwards cannot help,
because a deleted record can never acquire a hash. Pass `--komga-url` and the
check is done for you, and refuses rather than warns.

## How it works

Filenames from distributors are often squashed and unhelpful —
`hellboyomnibusvolume1_seedofdestruction.pdf`. Rather than guess at where the
words break, Comic Renamer looks the name up against a corpus of real published
comic titles — 229,836 series from the Grand Comics Database — and routes what
it cannot place to review rather than renaming it wrongly.

Measured against 2,717 held-out real filenames, the implemented resolver
locates the series for **91.6%**. Design-time research put it at 92.3%; the
implementation withdrew a tiebreaker that was holding part of that up, and the
gap is [recorded](CLAUDE.md) rather than rounded away.

The number that actually matters is the rate of files renamed confidently and
wrongly, because a wrong rename is the failure you do not notice. Against 2,223
filename-to-title pairs verified from a real Humble library, the research
measurement put it at **0.48%**, and the target is to stay at or under 0.5%.
That measurement has not yet been re-run against the shipped resolver, so treat
it as the design's number and not a claim about this binary.

## Prior art

[ComicTagger](https://github.com/comictagger/comictagger) already does filename
splitting, template renaming and metadata tagging, and does them well. The gap
this fills is narrower: **collected editions — omnibuses, trades, archives —
renamed from the filename alone, without opening multi-gigabyte files.**

## Credits

Comic data from the [Grand Comics Database](https://www.comics.org), licensed
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), via the
[paperbunkr GCD extract](https://github.com/heisehis/paperbunkr-gcd-data). The
GCD does not endorse this project.

## Licence

Code [MIT](LICENSE). Corpus data CC BY-SA 4.0, per its source.
