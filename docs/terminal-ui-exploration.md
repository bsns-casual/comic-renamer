# Terminal UI exploration — a framework

Status: exploration, back burner (2026-11-03). Nothing here is decided or built.
Source: the 2026-11-03 call between Mike and Rob. In `bsns-casual/brain`, see
`decisions/2026-11-03-comic-renamer-back-burner.md` (this doc is its first
follow-up), `meetings/2026-11-03-songlink-and-brain-setup.md`, and the raw
transcript `meetings/2026-11-03-nov3.md`, roughly lines 293–325.

**Recommendation:** sketch in a monospace Figma frame, prove the flow with a
throwaway `gum` script plus a VHS recording, and build the real thing in Rust
with `cliclack` (wizard prompts), keeping `ratatui` for the screens that need it.

---

## 1. How terminal UIs are built

A terminal UI is a grid of character cells. Every library either **prints prompts
one after another** (a wizard that scrolls up) or **redraws a full-screen grid**
(an app that owns the window, like `htop` or Claude Code).

| Library | Language | Model | Fits this repo? |
|---|---|---|---|
| **Ratatui** | Rust | Full-screen, immediate-mode redraw | **Already used**: `map`'s approval table and detail editor (`crates/cli/src/map/`) |
| **cliclack** / `inquire` / `dialoguer` | Rust | Sequential prompts: select, text, confirm, spinner | **Best fit for the wizard.** Same language, small dependency, no rewrite |
| Ink | JS/TS (React) | React components rendered to the terminal; what Claude Code uses | No. A second runtime and a Node install for a Rust binary |
| Bubble Tea (+ Huh, Lip Gloss) | Go | Elm-style model/update/view; Huh does forms | No for shipping. Its sibling **`gum`** is ideal for prototyping (§2) |
| Textual (+ Rich) | Python | CSS-styled widgets, can also run in a browser | No for shipping. Good for prototyping if Python is handy |

- The CLI is Rust (`clap`, `crossterm 0.28`, `ratatui 0.29`), and all the logic lives
  in `crates/core`. A new front end only has to call `scan`, `map` and `apply`
  logic that already exists.
- A wizard doesn't need to be full-screen. Use prompts for the questions and keep
  Ratatui for anything that needs scrolling or editing a list.
- Drag-and-drop already works. Dropping a Finder folder onto macOS Terminal or
  iTerm pastes its path (with spaces backslash-escaped), so "drag your folder
  here" is just a text prompt that unescapes the path.

## 2. How a designer can sketch and preview one

Work in three steps, from cheapest to most real.

| Step | Tool | What you get | Effort |
|---|---|---|---|
| **Sketch** | Figma frame using a monospace font (JetBrains Mono or SF Mono) on an 80×24 grid; or Monodraw / ASCIIFlow | Static screens with real character constraints | Minutes |
| **Click-through prototype** | `gum` shell script (`gum choose`, `gum input`, `gum confirm`, `gum spin`) with fake data | A flow you can actually run and hand to a tester | ~1 hour, no Rust |
| **Record a demo** | **VHS** (`.tape` script → GIF/MP4) or asciinema | A repeatable, shareable recording; re-render after each change | Minutes per take |

Rules for the Figma sketch:

- **Design on the grid.** Each character is one cell: 80 columns is the safe width,
  24 rows the safe height. You can't do half-cell spacing.
- **Colour is a hint, not structure.** Use 16 ANSI colours plus bold and dim, and
  the screen must still read in a monochrome or light theme.
- **Motion is frame-based.** Spinners, a progress bar, or a one-line "✓ 11 renamed"
  that ticks up. Rob's DMG-style "drop it in" moment becomes a line, not an
  animation.

A VHS tape *is* the spec. Commit it next to the prototype so Mike can replay the
exact flow. For example:

```
Output wizard.gif
Type "./prototype.sh" Enter
Sleep 1s  Down Enter          # pick Komga
Type "~/Comics" Enter
```

## 3. Proposed flow: a question-and-answer wizard

Each step asks one question and offers a default. Enter accepts it.

```
◆ Which library app?            ● Komga   ○ Kavita (not yet)
◆ Where is your library?        ~/Comics            (drag a folder here)
◆ Where are the new comics?     ~/Downloads/humble-bundle
◇ Found 15 files. 11 known, 4 new to me.
◆ What next?                    ● Preview first  ○ Rename now
   Preview:
     Saga Vol 1.cbz  →  Saga/Saga Vol. 01.cbz
     …10 more
◆ Rename these 11?              Yes / No
✓ Renamed 11.  4 couldn't be renamed. Let's name them.
◆ 1 of 4  mirkaandolfosmercy_vol1.pdf
   My guess: Mirka Andolfo's Mercy, Vol. 01
                                ● Use guess  ○ Edit  ○ Skip
…
✓ Renamed 3 more, skipped 1.   Undo with: comic-renamer undo
```

How this maps onto what exists, and what it must keep:

- **Steps 1–3** replace `--preset`, `-o` and the source argument. Remember the
  library path after the first run so returning users skip step 2.
- **Preview** is today's plan review, and **rename now** is `apply`. Show the
  preview in a Sonarr-style before → after list, not a table.
- **"Name the 4"** replaces the `map` approval table with one question per file.
  It must keep the rule that **nothing is stored without a keystroke**: the guess
  is the default answer, never applied silently, and "approve all remaining"
  (today's `^A`) is still offered and still reported at the end.
- **Copy or move** has to be asked or shown somewhere, since move is the default
  (D23). Ask once, on first run.
- The end screen is where Mike's contribution idea fits ("send these 3 new names
  to us?"), replacing today's commit-to-the-repo path with a CSV or zip (a "later"
  follow-up in the back-burner decision). Building it is a separate decision.

Open questions for a tester: is one question per unknown file tolerable at 40
unknowns? Should the wizard say what happened before the rename (preview by
default) or after (undo)?

## 4. Compared with a small drop-zone desktop app

| | Terminal wizard | Drop-zone desktop app |
|---|---|---|
| Who it's for | People already in a terminal, and Homebrew users | Anyone, including people who never open Terminal |
| Rob's workflow | New: grid sketches, `gum`, VHS. Figma's repo-to-design import doesn't help here, because there's no web UI to import | Familiar: Figma, and the build is HTML/CSS, so the repo-to-design import Rob tried on 2026-11-03 applies |
| Build | Add a prompt crate to the existing CLI. Days | Tauri app (Rust core plus web UI), with signing and notarisation. Weeks |
| Reuse | Calls `crates/core` directly | Calls `crates/core` through Tauri commands |
| Distribution | `brew install`, already working | DMG, Apple notarisation, a cask |
| Delight | Spinners, colour, one-liners | Logos, a drop target, animation |
| Risk to invariants | Low: same code paths as today | Medium: a new UI has to re-show the preview and approval steps |
| Cheap to explore? | **Yes**: prototype in an afternoon | Mockups are cheap; a real build isn't |

- **Do the wizard first.** It tests the actual question (does Q&A beat the table?)
  on the binary that already ships.
- **Hold the desktop app** until a tester who doesn't use a terminal asks for it.
  The wizard's question sequence becomes its screens almost one to one.

## Suggested next steps (time-boxed, back burner)

1. Rob sketches the wizard's ~6 screens in an 80×24 monospace Figma frame.
2. Turn it into a `gum` script with fake data, then record it with VHS.
3. Hand the script to one Humble buyer and note where they get stuck.
4. Only then decide on `cliclack` inside `crates/cli`.
