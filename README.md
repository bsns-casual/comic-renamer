# Comic Renamer

Rename comic files bought in bundles into the layout your library app expects.

Point it at a folder, pick your app, and it identifies each file and shows you
every proposed rename. Nothing moves until you approve the plan. Files it cannot
identify are flagged for review rather than guessed at.

**Status: `map`, `scan`, `apply`, `undo` and a startup menu work. Komga is the only preset, and
there is no Homebrew formula yet** — install from source, which is three
commands. See [`docs/specs/`](docs/specs/) for the design and
[`docs/research/`](docs/research/) for the measurements behind it.

## Install

**macOS only.** Not a packaging decision — the execution layer calls
`clonefile(2)`, `renamex_np` and `std::os::macos::fs`, with no `cfg` gating
and no fallback path, so there is nothing to build elsewhere. Developed and
tested on Apple Silicon.

Prerequisites:

```sh
# Rust 1.88 or newer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# A C compiler. `rusqlite` builds SQLite from source and `ring` compiles C,
# so rustup on its own is not enough.
xcode-select --install
```

To send mappings upstream, you also need the GitHub CLI
([`gh`](https://cli.github.com)), signed in with `gh auth login`. `map` does not
call `gh` itself: it prints the commands for you to run, and `gh` is what they
use. Nothing else needs it.

`command not found: cargo` means the first prerequisite is missing, or your
shell has not picked it up yet — open a new terminal, or `source ~/.cargo/env`.
An error out of `cc` or `libsqlite3-sys` means the second. Run `rustc --version`
before anything else. The `rust-version` in `Cargo.toml` is 1.88 because six
dependencies declare it, but an older toolchain does not report that. On 1.84.1
the build stops on a dependency that needs the `edition2024` Cargo feature, so
it reads like a broken dependency instead of an old compiler. (Measured on
1.84.1. Toolchains from 1.85 to 1.87 were not tried.)

Then:

```sh
git clone https://github.com/bsns-casual/comic-renamer
cd comic-renamer
cargo install --path crates/cli --locked
```

That puts `comic-renamer` in `~/.cargo/bin`, which rustup already adds to your
`PATH`. `cargo uninstall comic-renamer` removes it. The binary carries the
shipped mappings, the corpus manifest and the curated overrides compiled in, so
it works from any directory.

To run without installing, `cargo build --release` and use
`./target/release/comic-renamer`.

## Use it

```sh
# Name the files the database does not know, in a terminal session.
# Writes a plan and offers to run `apply` on it.
comic-renamer map ~/Bundles/humble-dark-horse --dest ~/Comics

# Or, with no session: plan what is mapped, list what is not. Touches nothing.
comic-renamer scan ~/Bundles/humble-dark-horse --dest ~/Comics   # add --copy to keep the originals

# Read the plan, fix what needs fixing, delete rows you do not want.
$EDITOR comic-renamer-plan.jsonl

# Move the files. Discloses the cost, then asks you to type MOVE.
comic-renamer apply comic-renamer-plan.jsonl

# Changed your mind. Reverses the most recent run.
comic-renamer undo

# Or see the runs that can be reversed, and pick one by the name shown.
comic-renamer undo --list
comic-renamer undo <run>

# With no arguments, open a menu that offers all of the above.
comic-renamer
```

`map` is the command for a bundle the tool has not seen. It looks every
filename up in a table of recorded titles. For each file it does not find, it
shows the filename, a guess, and a field for the name you approve. Nothing is
saved until you approve it. Approved names persist in
`~/Library/Application Support/comic-renamer` (or `$XDG_CONFIG_HOME/comic-renamer`).
At the end it writes a plan and hands it to `apply`, and offers to prepare a
contribution of your approved mappings (see below). When any file needs a name
from you, it refuses to run unless stdout is a terminal.

`scan` is the non-interactive path. It plans the files the table knows, lists
the ones it does not along with a guess for each, and saves nothing. A file with
no mapping stays where it is.

Shipped coverage is small today: 12 titles, from Fanatical, Global Comix and
Sweet Shop. For most bundles, `map` asks about every file. A much larger Humble
table exists as generated candidates and is held back for review (see
[`CLAUDE.md`](CLAUDE.md)).

A `map` or `scan` run with an unmapped Humble file downloads the corpus it
guesses from on first need — a 34 MiB archive, hash-verified against
[`corpus/manifest.json`](corpus/manifest.json) — and caches it in
`~/Library/Caches/comic-renamer`, where it unpacks to 97 MiB. A fully mapped
bundle never needs it, and a failed download means "no guess", not an error.

A file already at the destination is refused by name and left exactly as it is
— never overwritten, never renamed around. Merging into a library that already
holds some of these books is the ordinary case, not the awkward one. The
disclosure says which ones before you confirm, and a run that can place
nothing tells you why and exits non-zero rather than reporting success.

Editing the plan means editing the **metadata** — the series, the volume, the
subtitle. It does not mean editing paths: the destination is recomputed from
metadata every time, so a path typed into a plan file is ignored rather than
obeyed. That is deliberate, and it is what makes a plan safe to hand to an AI
assistant or attach to a pull request.

| Flag | On | Does |
|---|---|---|
| `--dest <dir>` | `map`, `scan` | Where the library goes. Required. |
| `--copy` | `map`, `scan` | Keeps the originals. Clones on APFS, so it usually costs nothing. |
| `-o <file>` | `map`, `scan` | Where to write the plan. Defaults to `comic-renamer-plan.jsonl`. |
| `--marketplace <name>` | `map`, `scan` | Where the files were bought. Decides how a guess is made. Defaults to `Humble`. |
| `--lang <code>` | `map`, `scan` | Which language's editions the corpus guess matches. Defaults to `en`, and never falls back to another. |
| `--komga-url <url>` | `apply` | Refuses unless your Komga can survive a rename. Token from `COMIC_RENAMER_KOMGA_TOKEN`. |
| `--yes` | `apply` | Skips the typed confirmation. The disclosure still prints. |
| `--allow-cloud-move` | `apply` | Permits a move out of a cloud-sync folder, where it is a server-side delete. |

`undo` takes a run, not a plan. Passing it a plan path is an error. A run is
recorded as it goes, in a directory the tool keeps for itself, and `undo` reads
only that directory. When a run copied files in, `undo`
removes a library file only if the file it was copied from is still where it
was, with the same content. If you deleted the bundle folder after a copy,
`undo` refuses and says so, because the library file would be the last copy.
It takes no `--yes`; it has nothing to confirm.

`comic-renamer` with no arguments opens a menu and needs a terminal. If an
earlier run was interrupted, or a finished one is still reversible, the menu
offers to reverse it first. Press `q` to quit. Quitting declines the undo of a
run you started in that session, and it is deleted.

**Moving is the default, and it is destructive by design** — a same-volume move
is a `rename`, which costs nothing and cannot truncate a multi-gigabyte PDF
halfway. Every run records what it did as it goes, in a directory the tool
keeps for itself, so an interrupted run is still reversible.

### Before you point it at a Komga library

A rename is a delete-plus-add to Komga, which rematches by size and then by
hash. **Hashing has to already be on** — switching it on afterwards cannot help,
because a deleted record can never acquire a hash. Pass `--komga-url` and the
check is done for you, and refuses rather than warns.

Komga is the only preset, and its layout does not suit Kavita. Komga names a
series from its folder; Kavita names it from the filename, so the Komga layout
splits a Kavita library into one-book series for roughly 40% of a Humble bundle.
This was read from Kavita's source, not tested against a running Kavita. See
[`docs/research/2026-10-23-kavita-from-source.md`](docs/research/2026-10-23-kavita-from-source.md).

## How it works

Comic marketplaces publish the real title next to every download, so the title
is a known quantity. The filename is often squashed and unhelpful —
`hellboyomnibusvolume1_seedofdestruction.pdf` — and sometimes the characters
that carry the title are already gone. Fanatical ships
`HackSlash Back to School.cbz` for *Hack/Slash: Back to School*. No inference
recovers a slash that is not in the input.

So the tool looks a filename up instead of working out its title. A row in the
mapping table is keyed by the filename with Unicode normalised, case folded and
every non-alphanumeric character removed, and carries the series, volume and
subtitle. That key rule is frozen: changing it invalidates every row ever
contributed. One row serves every format of a comic, so the `.cbz`, `.pdf` and
`.epub` of one title share it.

When a file has no row, the tool offers a guess to start from. For Humble
bundles the guess comes from a corpus of published comic titles, the Grand
Comics Database, and for the other marketplaces from the filename itself. A
guess is a typing aid. It is never stored without your approval.

Measured against the 2,223 verified Humble filename-to-title pairs in
[`fixtures/humble-ground-truth.tsv`](fixtures/humble-ground-truth.tsv), the
guess's series is right 62.9% of the time and its full title 48.9% of the time.
The corpus locates the series for 91.6% of 2,717 held-out filenames.
`cargo xtask measure` reports these. None is a pass-or-fail threshold: a wrong
guess costs you some typing, not a misfiled file.

## Contributing mappings

A mapping row holds metadata only — never a path. When you say yes at the end of
a `map` session, it writes the rows you approved to a file under
`~/Library/Application Support/comic-renamer/contributions` and prints the
`gh` and `git` commands that fork the repository and open a pull request. It
runs none of them. Rows you corrected and chose not to contribute stay local.

## Prior art

[ComicTagger](https://github.com/comictagger/comictagger) already does filename
splitting, template renaming and metadata tagging, and does them well. The gap
this fills is narrower: **collected editions — omnibuses, trades, archives —
renamed from the filename alone, without opening multi-gigabyte files.**

## Credits

Guessing data from the [Grand Comics Database](https://www.comics.org), licensed
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), via the
[paperbunkr GCD extract](https://github.com/heisehis/paperbunkr-gcd-data). The
GCD does not endorse this project.

## Licence

Code [MIT](LICENSE). Corpus data CC BY-SA 4.0, per its source. The licence for
publishing titles taken from Humble's order API as a dataset is not settled;
that question gates a release.
