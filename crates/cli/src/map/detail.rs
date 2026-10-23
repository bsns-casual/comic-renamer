//! The detail editor: one title sentence, parsed live, with the path it would
//! produce shown underneath.
//!
//! The destination line is the safety mechanism. It is rendered by the real
//! preset from the fields as they stand, so a mis-parse is visible before
//! anything is stored. The editor persists nothing; it hands back fields.

use super::table::{show, Action, Entry};
use comic_renamer_core::mapping::MappingRow;
use comic_renamer_core::preset::Preset;
use comic_renamer_core::title::{parse_title, ParsedTitle};
use comic_renamer_core::types::Suggestion;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::{DefaultTerminal, Frame};

pub struct Detail {
    pub text: String,
    pub parsed: ParsedTitle,
    candidates: Vec<Suggestion>,
    /// The candidate whose text the user is editing from. Its `volume_count`
    /// sets the padding, but only while the series still matches it.
    picked: Option<usize>,
    /// The file's real extension, so the destination is the path `apply` will make.
    pub ext: String,
    /// `None` is the sentence box; otherwise one field is being edited directly.
    field: Option<Field>,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Series,
    Volume,
    Subtitle,
}

impl Detail {
    pub fn from_text(text: &str) -> Detail {
        Detail { text: text.to_string(), parsed: parse_title(text), candidates: vec![], picked: None, ext: String::new(), field: None }
    }

    /// Starts from what the user already typed, else the top candidate, else
    /// an empty box.
    fn for_entry(entry: &Entry) -> Detail {
        let text = match (&entry.final_fields, entry.guess()) {
            (Some(f), _) => show(&f.series, f.volume, f.subtitle.as_deref()),
            (None, Some(g)) => show(&g.series, g.volume, g.subtitle.as_deref()),
            (None, None) => String::new(),
        };
        let candidates = entry.candidates().to_vec();
        let picked = (!candidates.is_empty()).then_some(0);
        Detail { picked, candidates, ext: entry.extension.clone(), ..Detail::from_text(&text) }
    }

    fn reparse(&mut self) {
        self.parsed = parse_title(&self.text);
    }

    /// Tab: sentence box, then series, volume, subtitle, then back.
    fn next_field(&mut self) {
        self.field = match self.field {
            None => Some(Field::Series),
            Some(Field::Series) => Some(Field::Volume),
            Some(Field::Volume) => Some(Field::Subtitle),
            Some(Field::Subtitle) => None,
        };
    }

    /// Edits one field and rewrites the sentence from the fields. The others
    /// are never re-parsed, so a field edit cannot move text between them.
    fn edit_field(&mut self, f: Field, c: Option<char>) {
        let p = &mut self.parsed;
        match f {
            Field::Series => {
                match c { Some(c) => p.series.push(c), None => { p.series.pop(); } }
            }
            Field::Volume => {
                let mut v = p.volume.map(|v| v.to_string()).unwrap_or_default();
                match c {
                    Some(c) if c.is_ascii_digit() => v.push(c),
                    Some(_) => return,
                    None => { v.pop(); }
                }
                p.volume = v.parse().ok();
            }
            Field::Subtitle => {
                let mut t = p.subtitle.take().unwrap_or_default();
                match c { Some(c) => t.push(c), None => { t.pop(); } }
                p.subtitle = (!t.is_empty()).then_some(t);
            }
        }
        self.text = show(&p.series, p.volume, p.subtitle.as_deref());
    }

    pub fn on_input(&mut self, c: char) {
        if let Some(f) = self.field {
            return self.edit_field(f, Some(c));
        }
        self.text.push(c);
        self.reparse();
    }

    fn on_backspace(&mut self) {
        if let Some(f) = self.field {
            return self.edit_field(f, None);
        }
        self.text.pop();
        self.reparse();
    }

    /// Moves the menu and loads that candidate into the text box, where it can
    /// be edited like anything typed.
    fn pick(&mut self, delta: isize) {
        let Some(last) = self.candidates.len().checked_sub(1) else { return };
        let at = (self.picked.unwrap_or(0) as isize + delta).clamp(0, last as isize) as usize;
        self.picked = Some(at);
        self.field = None;
        let c = &self.candidates[at];
        self.text = show(&c.series, c.volume, c.subtitle.as_deref());
        self.reparse();
    }

    /// The picked candidate's count pads the volume, but only for that series:
    /// retyping to another one must not inherit a width it has no reason to have.
    fn volume_count(&self) -> Option<u32> {
        let c = &self.candidates[self.picked?];
        (c.series == self.parsed.series).then_some(c.volume_count).flatten()
    }

    /// The path the current fields produce, from the real preset.
    pub fn destination_line(&self, preset: &dyn Preset, ext: &str) -> String {
        if self.parsed.series.trim().is_empty() {
            return "(type a series name)".to_string();
        }
        let row = MappingRow {
            key: String::new(),
            series: self.parsed.series.clone(),
            volume: self.parsed.volume,
            subtitle: self.parsed.subtitle.clone(),
            lang: String::new(),
            volume_count: self.volume_count(),
            bundle: String::new(),
        };
        preset.destination(&row.resolved(), ext).display().to_string()
    }

    fn on_key(&mut self, k: KeyEvent) -> Step {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('c') if ctrl => Step::Quit,
            KeyCode::Esc => Step::Back,
            KeyCode::Enter if !self.parsed.series.trim().is_empty() => Step::Accept,
            KeyCode::Tab => {
                self.next_field();
                Step::Editing
            }
            KeyCode::Up => {
                self.pick(-1);
                Step::Editing
            }
            KeyCode::Down => {
                self.pick(1);
                Step::Editing
            }
            KeyCode::Backspace => {
                self.on_backspace();
                Step::Editing
            }
            KeyCode::Char(c) if !ctrl => {
                self.on_input(c);
                Step::Editing
            }
            _ => Step::Editing,
        }
    }

    fn render(&self, frame: &mut Frame, stem: &str, preset: &dyn Preset) {
        let [head, hint, input, parsed, dest, menu, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(format!(" {stem}"), head);
        frame.render_widget("\n type a title, or press tab to edit a field; check the path below", hint);
        let boxed = ratatui::widgets::Block::bordered();
        frame.render_widget(ratatui::widgets::Paragraph::new(format!("{}▏", self.text)).block(boxed), input);
        let p = &self.parsed;
        let mark = |f| if self.field == Some(f) { '>' } else { ' ' };
        frame.render_widget(
            format!(
                " parsed as\n  {}series   {}\n  {}volume   {}\n  {}subtitle {}",
                mark(Field::Series),
                p.series,
                mark(Field::Volume),
                p.volume.map_or("(none)".to_string(), |v| v.to_string()),
                mark(Field::Subtitle),
                p.subtitle.as_deref().unwrap_or("(none)"),
            ),
            parsed,
        );
        frame.render_widget(
            format!("\n files to   {}", self.destination_line(preset, &self.ext)),
            dest,
        );
        if self.candidates.len() > 1 {
            let lines: Vec<String> = std::iter::once(" our candidates (↑↓ to pick)".to_string())
                .chain(self.candidates.iter().enumerate().map(|(i, c)| {
                    let mark = if self.picked == Some(i) { '>' } else { ' ' };
                    format!("  {mark} {}", show(&c.series, c.volume, c.subtitle.as_deref()))
                }))
                .collect();
            frame.render_widget(lines.join("\n"), menu);
        }
        frame.render_widget(
            ratatui::widgets::Paragraph::new(" ⏎ accept   tab next field   ↑↓ candidates   esc back to the table   ^C quit")
                .style(Style::new().add_modifier(Modifier::DIM)),
            foot,
        );
    }
}

enum Step {
    Editing,
    Accept,
    Back,
    Quit,
}

/// Edits one entry. `Continue` returns to the table (accepted or not);
/// `Cancel` means the user quit the whole session.
pub fn run(terminal: &mut DefaultTerminal, entry: &mut Entry, preset: &dyn Preset) -> std::io::Result<Action> {
    let mut detail = Detail::for_entry(entry);
    loop {
        terminal.draw(|f| detail.render(f, &entry.stem, preset))?;
        let Event::Key(key) = crossterm::event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match detail.on_key(key) {
            Step::Editing => {}
            Step::Back => return Ok(Action::Continue),
            Step::Quit => return Ok(Action::Cancel),
            Step::Accept => {
                if entry.accept_edit(detail.parsed.clone()) {
                    return Ok(Action::Continue);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use comic_renamer_core::preset::komga::Komga;
    use comic_renamer_core::types::SuggestionSource;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sug(series: &str, volume: Option<u32>, subtitle: Option<&str>, count: Option<u32>) -> Suggestion {
        Suggestion {
            series: series.into(),
            volume,
            subtitle: subtitle.map(String::from),
            volume_count: count,
            source: SuggestionSource::Corpus,
        }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn screen(d: &Detail, stem: &str) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
        term.draw(|f| d.render(f, stem, &Komga)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn typing_a_sentence_shows_the_parsed_fields() {
        let d = Detail::from_text("The Complete ElfQuest Volume 1: The Original Quest TPB");
        assert_eq!(d.parsed.series, "The Complete ElfQuest");
        assert_eq!(d.parsed.volume, Some(1));
        assert_eq!(d.parsed.subtitle.as_deref(), Some("The Original Quest TPB"));
    }

    #[test]
    fn every_keystroke_reparses() {
        let mut d = Detail::from_text("DIE");
        for c in " Volume 3".chars() {
            d.on_input(c);
        }
        assert_eq!((d.parsed.series.as_str(), d.parsed.volume), ("DIE", Some(3)));
        d.on_backspace();
        d.on_backspace();
        assert_eq!(d.text, "DIE Volume");
        assert_eq!((d.parsed.series.as_str(), d.parsed.volume), ("DIE Volume", None));
    }

    #[test]
    fn the_parser_keeps_the_users_string_verbatim() {
        assert_eq!(Detail::from_text("DIE: Loaded #1").parsed.series, "DIE");
        assert_eq!(Detail::from_text("Hack/Slash: Back to School").parsed.series, "Hack/Slash");
    }

    #[test]
    fn the_destination_line_is_rendered_by_the_real_preset() {
        let d = Detail::from_text("Hack/Slash: Back to School");
        let line = d.destination_line(&Komga, "cbz");
        assert!(line.contains("Hack - Slash"), "the slash is sanitised by the preset, not stored mangled: {line}");
        assert_eq!(line.matches('/').count(), 1, "only the series/book separator: {line}");
    }

    #[test]
    fn the_destination_line_matches_the_spec_example() {
        let d = Detail::from_text("The Complete ElfQuest Volume 1: The Original Quest TPB");
        assert_eq!(
            d.destination_line(&Komga, "cbz"),
            "The Complete ElfQuest/The Complete ElfQuest Vol. 01 - The Original Quest TPB.cbz"
        );
    }

    #[test]
    fn padding_comes_from_volume_count_not_from_the_volume() {
        let d = Detail::for_entry(&Entry::new("s".into(), vec![sug("Manga", Some(2), None, Some(100))]));
        let line = d.destination_line(&Komga, "cbz");
        assert!(line.contains("Vol. 002"), "{line}");
        let d = Detail::from_text("Manga Volume 2");
        let line = d.destination_line(&Komga, "cbz");
        assert!(line.contains("Vol. 02."), "no count means the two-digit floor: {line}");
    }

    #[test]
    fn retyping_the_series_drops_the_picked_candidates_count() {
        let mut d = Detail::for_entry(&Entry::new("s".into(), vec![sug("Manga", Some(2), None, Some(100))]));
        d.text = "Other Volume 2".into();
        d.on_input('0');
        let line = d.destination_line(&Komga, "cbz");
        assert!(line.contains("Vol. 20."), "{line}");
    }

    #[test]
    fn a_blank_title_has_no_destination() {
        let line = Detail::from_text("").destination_line(&Komga, "cbz");
        assert!(!line.contains('/') && !line.contains(".cbz"), "{line}");
    }

    #[test]
    fn candidates_are_a_menu_that_loads_into_the_text_box() {
        let e = Entry::new(
            "witch".into(),
            vec![sug("Witch", Some(1), None, None), sug("Witcher", Some(1), None, None)],
        );
        let mut d = Detail::for_entry(&e);
        assert_eq!(d.parsed.series, "Witch");
        d.pick(1);
        assert_eq!(d.parsed.series, "Witcher");
        assert_eq!(d.text, "Witcher Volume 1");
        d.pick(1);
        assert_eq!(d.parsed.series, "Witcher", "clamps at the last candidate");
        let s = screen(&d, "witch");
        assert!(s.contains("Witch Volume 1") && s.contains("Witcher Volume 1"), "{s}");
    }

    #[test]
    fn the_screen_shows_the_fields_and_the_destination() {
        let mut d = Detail::from_text("The Complete ElfQuest Volume 1: The Original Quest TPB");
        d.ext = "cbz".into();
        let s = screen(&d, "thecompleteelfquest_vol1");
        for want in [
            "thecompleteelfquest_vol1",
            "series   The Complete ElfQuest",
            "volume   1",
            "subtitle The Original Quest TPB",
            "files to   The Complete ElfQuest/The Complete ElfQuest Vol. 01 - The Original Quest TPB.cbz",
        ] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
    }

    #[test]
    fn enter_accepts_only_a_title_with_a_series() {
        let mut d = Detail::from_text("");
        assert!(matches!(d.on_key(key(KeyCode::Enter)), Step::Editing), "a blank title is not accepted");
        d.on_input('X');
        assert!(matches!(d.on_key(key(KeyCode::Enter)), Step::Accept));
        assert!(matches!(d.on_key(key(KeyCode::Esc)), Step::Back));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(d.on_key(ctrl_c), Step::Quit));
    }

    #[test]
    fn an_edited_squashed_row_is_approved_through_the_one_writer() {
        let mut e = Entry::new("x".into(), vec![sug("Hellboy Omnibus", Some(1), Some("seedofdestruction"), None)]);
        assert!(!e.approved);
        let d = Detail::from_text("Hellboy Omnibus Volume 1: Seed of Destruction");
        assert!(e.accept_edit(d.parsed.clone()));
        assert!(e.approved);
        assert_eq!(e.final_fields.unwrap().subtitle.as_deref(), Some("Seed of Destruction"));
    }

    #[test]
    fn a_blank_series_is_never_accepted() {
        let mut e = Entry::new("x".into(), vec![]);
        assert!(!e.accept_edit(Detail::from_text("   ").parsed));
        assert!(!e.approved && e.final_fields.is_none());
    }

    #[test]
    fn the_guess_is_always_the_first_candidate() {
        let e = Entry::new("x".into(), vec![sug("A", None, None, None), sug("B", None, None, None)]);
        assert_eq!(e.guess().unwrap().series, "A");
        assert!(Entry::new("x".into(), vec![]).guess().is_none());
    }

    fn type_str(d: &mut Detail, s: &str) {
        for c in s.chars() {
            d.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn a_pdf_entry_shows_a_pdf_destination() {
        let e = Entry::new("elfquest_vol1".into(), vec![sug("ElfQuest", Some(1), None, None)]).with_extension("pdf");
        let d = Detail::for_entry(&e);
        let s = screen(&d, &e.stem);
        assert!(s.contains("ElfQuest Vol. 01.pdf"), "{s}");
        assert!(!s.contains(".cbz"), "the tool will not create a .cbz: {s}");
    }

    #[test]
    fn tab_edits_one_field_and_leaves_the_others_intact() {
        let mut d = Detail::from_text("Series Volume 1: Sub");
        d.on_key(key(KeyCode::Tab)); // series
        d.on_key(key(KeyCode::Tab)); // volume
        d.on_key(key(KeyCode::Backspace));
        type_str(&mut d, "7x"); // non-digits are ignored in the volume field
        assert_eq!(d.parsed.volume, Some(7));
        assert_eq!(d.parsed.series, "Series");
        assert_eq!(d.parsed.subtitle.as_deref(), Some("Sub"));
        assert_eq!(d.text, "Series Volume 7: Sub");
        assert!(d.destination_line(&Komga, "pdf").contains("Series Vol. 07 - Sub.pdf"));
    }

    #[test]
    fn a_field_can_be_cleared_and_the_subtitle_removed() {
        let mut d = Detail::from_text("Series Volume 1: Sub");
        for _ in 0..3 {
            d.on_key(key(KeyCode::Tab));
        }
        for _ in 0..3 {
            d.on_key(key(KeyCode::Backspace));
        }
        assert_eq!(d.parsed.subtitle, None);
        assert_eq!(d.text, "Series Volume 1");
    }

    #[test]
    fn tab_cycles_back_to_the_text_box_and_the_footer_is_honest() {
        let mut d = Detail::from_text("A Volume 2");
        for _ in 0..4 {
            d.on_key(key(KeyCode::Tab));
        }
        type_str(&mut d, "!");
        assert_eq!(d.text, "A Volume 2!");
        assert!(screen(&d, "x").contains("tab"), "the footer advertises a key that works");
    }
}
