#!/usr/bin/env bash
# A throwaway click-through prototype of the comic-renamer wizard.
#
# Fake data only: it reads no folders and renames nothing. It exists to put the
# question-and-answer flow in front of a tester before any Rust is written.
#
#   flow     docs/terminal-ui-exploration.md, section 3
#   screens  Figma "comic-renamer", page "Terminal grid"
#   needs    gum   (brew install gum)
#   run      ./prototype/wizard.sh
set -euo pipefail

command -v gum >/dev/null || { echo "This prototype needs gum: brew install gum" >&2; exit 1; }

# ANSI colours 0-15 only, never gum's 256-colour defaults, so the terminal's
# own light or dark theme recolours every screen.
export GUM_CHOOSE_CURSOR="● " GUM_CHOOSE_CURSOR_FOREGROUND=2 GUM_CHOOSE_HEADER_FOREGROUND=6
export GUM_INPUT_PROMPT="│  " GUM_INPUT_PROMPT_FOREGROUND=6 GUM_INPUT_CURSOR_FOREGROUND=6
export GUM_INPUT_HEADER_FOREGROUND=6 GUM_INPUT_PLACEHOLDER_FOREGROUND=8
export GUM_CONFIRM_PROMPT_FOREGROUND=6
export GUM_CONFIRM_SELECTED_FOREGROUND=0 GUM_CONFIRM_SELECTED_BACKGROUND=6
export GUM_CONFIRM_UNSELECTED_FOREGROUND=7 GUM_CONFIRM_UNSELECTED_BACKGROUND=8
export GUM_SPIN_SPINNER_FOREGROUND=6
# gum 2 strips all styling when its output is captured, which every $(green …)
# below does. Keep it when the script itself is on a terminal, unless NO_COLOR.
[ -t 1 ] && [ -z "${NO_COLOR:-}" ] && export CLICOLOR_FORCE=1

# --- fake data ---------------------------------------------------------------
KNOWN=(  # "bundle filename|what the Komga preset would name it"
  "Saga Vol 1.cbz|Saga/Saga Vol. 01.cbz"
  "Saga Vol 2.cbz|Saga/Saga Vol. 02.cbz"
  "Saga Vol 3.cbz|Saga/Saga Vol. 03.cbz"
  "Paper Girls Vol 1.cbz|Paper Girls/Paper Girls Vol. 01.cbz"
  "Paper Girls Vol 2.cbz|Paper Girls/Paper Girls Vol. 02.cbz"
  "Monstress Vol 1.cbz|Monstress/Monstress Vol. 01.cbz"
  "Monstress Vol 2.cbz|Monstress/Monstress Vol. 02.cbz"
  "Descender Vol 1.cbz|Descender/Descender Vol. 01.cbz"
  "Lazarus Vol 1.cbz|Lazarus/Lazarus Vol. 01.cbz"
  "Black Science Vol 1.cbz|Black Science/Black Science Vol. 01.cbz"
  "East of West Vol 1.cbz|East of West/East of West Vol. 01.cbz"
)
UNKNOWN=(  # "bundle filename|the resolver's guess" -- the last guess is wrong on purpose
  "mirkaandolfosmercy_vol1.pdf|Mirka Andolfo's Mercy, Vol. 01"
  "usagiyojimbo_thespecialedition.cbz|Usagi Yojimbo: The Special Edition"
  "maggiethemechanic_loveandrockets.cbz|Maggie the Mechanic (Love & Rockets)"
  "21_thestoryofrobertoclementepb.pdf|21"
)

# --- drawing -----------------------------------------------------------------
dim()   { gum style --faint -- "$*"; }
bold()  { gum style --bold -- "$*"; }
green() { gum style --foreground 2 -- "$*"; }
yellow(){ gum style --foreground 3 -- "$*"; }
gap()   { dim "│"; }
# An answered question collapses to one line plus its answer, as in the Figma frames.
answered() { echo "$(green '◇')  $1"; echo "$(dim '│')  $(dim "$2")"; gap; }
note()     { echo "$(green "${2:-◇}")  $1"; gap; }

# Folders dragged onto Terminal or iTerm arrive backslash-escaped, often with a
# trailing space. Undo both; type ~ by hand and it stays ~.
unescape_path() {
  local p="$1"
  p="${p#"${p%%[![:space:]]*}"}"; p="${p%"${p##*[![:space:]]}"}"
  printf '%s' "$p" | sed -e 's/\\\(.\)/\1/g'
}

ask_path() {  # question, default
  local raw
  raw=$(gum input --header "◆  $1" --value "$2" --placeholder "drag a folder here, or type a path")
  unescape_path "$raw"
}

# The closing burst, as in the Figma frames "Step 7 · motion". Raw ANSI codes
# rather than gum style: 28 gum calls would be slower than the animation.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  Y=$'\033[33m' R=$'\033[1;31m' C=$'\033[36m' D=$'\033[2m' N=$'\033[0m'
else
  Y='' R='' C='' D='' N=''
fi
burst() {  # frame 1-4: the word, then its box, then the inner rays, then all of it
  local f=$1 g="${D}│${N}" k='' word="${R}K A P O W !${N}" r
  [ -t 1 ] && k=$'\033[K'
  local box="${Y}|${N}  ${word}  ${Y}|${N}"
  local r0='' r1='' r2='' r3="        ${word}" r4='' r5='' r6=''
  if [ "$f" -ge 2 ]; then
    r2="     ${Y}.--'''''''''''--.${N}"; r3="     $box"; r4="     ${Y}'--...........--'${N}"
  fi
  if [ "$f" -ge 3 ]; then
    r1="         ${Y}\\   |   /${N}"; r5="         ${Y}/   |   \\${N}"
    r3="  ${Y}--${N} $box ${Y}--${N}"
  fi
  if [ "$f" -ge 4 ]; then
    r0="      ${Y}\\      |      /${N}"; r6="      ${Y}/      |      \\${N}"
    r1="   ${C}*${N}     ${Y}\\   |   /${N}     ${C}*${N}"
    r5="   ${C}.${N}     ${Y}/   |   \\${N}     ${C}.${N}"
  fi
  for r in "$r0" "$r1" "$r2" "$r3" "$r4" "$r5" "$r6"; do printf '%s%s%s\n' "$g" "$r" "$k"; done
}
celebrate() {  # about 0.6s of frames on a terminal, then the last one stays
  local f
  if [ -t 1 ]; then
    printf '\033[?25l'
    for f in 1 2 3; do burst "$f"; sleep 0.2; printf '\033[7A'; done
  fi
  burst 4
  [ -t 1 ] && printf '\033[?25h'
  return 0
}

finished=0
trap '[ -t 1 ] && printf "\033[?25h"; [ "$finished" = 1 ] || { echo; dim "└  Stopped. Nothing was renamed."; }' EXIT

# --- the wizard --------------------------------------------------------------
echo "$(dim '┌')  $(bold comic-renamer)"
gap

# 1. Which library app?
while :; do
  app=$(gum choose --header "◆  Which library app?" "Komga" "Kavita (not yet)")
  [ "$app" = "Komga" ] && break
  echo "$(yellow '▲')  Kavita isn't supported yet. Komga is the only layout today."
  gap
done
answered "Which library app?" "$app"

# 2. Where is your library?
library=$(ask_path "Where is your library?" "~/Comics")
answered "Where is your library?" "$library"

# 3. Where are the new comics?
source=$(ask_path "Where are the new comics?" "~/Downloads/humble-bundle")
answered "Where are the new comics?" "$source"

# 4. Found N files. What next?
# Braces matter: macOS's bash 3.2 reads the first byte of "…" as part of a bare $name.
gum spin --title "Looking in ${source}…" -- sleep 1
known=${#KNOWN[@]} new=${#UNKNOWN[@]}
note "Found $((known + new)) files. $(green "$known known"), $(yellow "$new new to me")."

next=$(gum choose --header "◆  What next?" "Preview first" "Rename now")

# 5. Preview, then confirm. "Rename now" is itself the keystroke, so it skips both.
if [ "$next" = "Preview first" ]; then
  echo "$(green '◇')  What next?"
  echo "$(dim '│')  $(dim 'Preview first')"
  first=${KNOWN[0]}
  echo "$(dim '│')  ${first%%|*}  $(dim '→')  $(green "${first#*|}")"
  echo "$(dim '│')  $(dim "…$((known - 1)) more")"
  gap
  if ! gum confirm "Rename these $known?"; then
    finished=1
    dim "└  Nothing renamed."
    exit 0
  fi
  answered "Rename these $known?" "Yes"
else
  answered "What next?" "Rename now"
fi

gum spin --title "Renaming $known files…" -- sleep 1
note "Renamed $known.  $(yellow "$new couldn't be renamed.") Let's name them." "✓"

# 6. One question per unknown file. The guess is the default, never applied
# silently, and "use all remaining" is reported at the end (today's ^A).
renamed=0 skipped=0 all_from=0 i=0
for entry in "${UNKNOWN[@]}"; do
  i=$((i + 1))
  file=${entry%%|*} guess=${entry#*|}
  if [ "$all_from" -gt 0 ]; then
    choice="Use guess"
  else
    choice=$(gum choose \
      --header "$(printf '◆  %s of %s  %s\n   My guess: %s' "$i" "$new" "$file" "$guess")" \
      "Use guess" "Edit" "Skip" "Use all remaining guesses")
    [ "$choice" = "Use all remaining guesses" ] && { all_from=$i; choice="Use guess"; }
  fi
  case $choice in
    "Use guess") name=$guess ;;
    "Edit")      name=$(gum input --header "◆  Name for $file" --value "$guess") ;;
    "Skip")      name="" ;;
  esac
  echo "$(green '◇')  $i of $new  $file"
  if [ -n "$name" ]; then
    renamed=$((renamed + 1)); echo "$(dim '│')  $(dim "$name")"
  else
    skipped=$((skipped + 1)); echo "$(dim '│')  $(dim 'skipped')"
  fi
  gap
done

celebrate
gap
echo "$(green '✓')  $((known + renamed)) comics shelved in ${library}. $skipped skipped."
[ "$all_from" -gt 0 ] && echo "$(dim '│')  $(dim "$((new - all_from + 1)) named by \"Use all remaining guesses\".")"
finished=1
echo "$(dim '└')  Undo with: $(bold 'comic-renamer undo')"
