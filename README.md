# Comic Renamer

Rename comic files bought in bundles into the layout your library app expects.

Point it at a folder, pick your app, and it identifies each file and shows you
every proposed rename. Nothing moves until you approve the plan. Files it cannot
identify are flagged for review rather than guessed at.

**Status: design complete, no implementation yet.** See
[`docs/specs/`](docs/specs/) for the design and
[`docs/research/`](docs/research/) for the measurements behind it.

## How it works

Filenames from distributors are often squashed and unhelpful —
`hellboyomnibusvolume1_seedofdestruction.pdf`. Rather than guess at where the
words break, Comic Renamer looks the name up against a corpus of real published
comic titles. On a held-out set of 2,714 real filenames it located the series for
**92.3%**, and routed the rest to review rather than renaming them wrongly.

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
