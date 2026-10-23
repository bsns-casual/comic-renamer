//! The three-column approval table: filename, our guess, final name.
//!
//! Nothing here persists anything. The table edits `Entry` values in memory;
//! writing approved rows to the local mapping file is a separate step.

use comic_renamer_core::title::ParsedTitle;
use comic_renamer_core::types::{Suggestion, SuggestionSource};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Cell, Row, TableState};
use ratatui::Frame;

pub struct Entry {
    pub stem: String,
    pub guess: Option<Suggestion>,
    /// What the user typed or accepted. `None` until one of those happens.
    pub final_fields: Option<ParsedTitle>,
    pub approved: bool,
}

impl Entry {
    /// A corpus hit that carries a subtitle is raw stem text the resolver could
    /// not split (`seedofdestruction`). The series is right and the subtitle is
    /// garbled, so the guess is not safe to accept unread. Same discriminator
    /// as `a_squashed_subtitle_is_identifiable_on_the_suggestion`.
    pub fn has_squashed_subtitle(&self) -> bool {
        self.guess
            .as_ref()
            .is_some_and(|g| g.source == SuggestionSource::Corpus && g.subtitle.is_some())
    }

    /// The guess as final fields, when it is safe to take without a human edit.
    fn clean_guess(&self) -> Option<ParsedTitle> {
        let g = self.guess.as_ref().filter(|_| !self.has_squashed_subtitle())?;
        Some(ParsedTitle { series: g.series.clone(), volume: g.volume, subtitle: g.subtitle.clone() })
    }

    /// Approves with the user's fields, else the clean guess. Returns false
    /// when there is nothing safe to approve.
    fn approve(&mut self) -> bool {
        if self.final_fields.is_none() {
            let Some(fields) = self.clean_guess() else { return false };
            self.final_fields = Some(fields);
        }
        self.approved = true;
        true
    }
}

pub struct Table {
    pub folder: String,
    pub entries: Vec<Entry>,
    pub cursor: usize,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Continue,
    OpenDetail(usize),
    Done,
    Cancel,
}

fn show(series: &str, volume: Option<u32>, subtitle: Option<&str>) -> String {
    let mut s = series.to_string();
    if let Some(v) = volume {
        s.push_str(&format!(" Volume {v}"));
    }
    if let Some(t) = subtitle {
        s.push_str(&format!(": {t}"));
    }
    s
}

impl Table {
    pub fn new(folder: String, entries: Vec<Entry>) -> Self {
        Table { folder, entries, cursor: 0 }
    }

    fn approved_count(&self) -> usize {
        self.entries.iter().filter(|e| e.approved).count()
    }

    /// `^A`: approve every row that has something safe to approve. A row with
    /// a squashed subtitle is skipped unless the user already edited it.
    fn approve_all_remaining(&mut self) {
        for e in self.entries.iter_mut().filter(|e| !e.approved) {
            e.approve();
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        let last = self.entries.len().saturating_sub(1);
        match (k.code, k.modifiers.contains(KeyModifiers::CONTROL)) {
            (KeyCode::Char('a'), true) => self.approve_all_remaining(),
            (KeyCode::Char('s'), true) => return Action::Done,
            (KeyCode::Esc | KeyCode::Char('q'), false) => return Action::Cancel,
            (KeyCode::Up, _) => self.cursor = self.cursor.saturating_sub(1),
            (KeyCode::Down, _) => self.cursor = (self.cursor + 1).min(last),
            (KeyCode::Char('e'), false) if !self.entries.is_empty() => return Action::OpenDetail(self.cursor),
            (KeyCode::Enter, _) => {
                let Some(e) = self.entries.get_mut(self.cursor) else { return Action::Continue };
                if e.approved {
                    e.approved = false;
                } else if !e.approve() && e.has_squashed_subtitle() {
                    // Enter never accepts a squashed guess; it sends the user
                    // to fix it.
                    return Action::OpenDetail(self.cursor);
                }
            }
            _ => {}
        }
        Action::Continue
    }

    pub fn render(&self, frame: &mut Frame) {
        let [head, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        let title = format!("Unmapped files in \"{}\"", self.folder);
        let count = format!("{} of {} approved", self.approved_count(), self.entries.len());
        let pad = (head.width as usize).saturating_sub(title.chars().count() + count.chars().count() + 2);
        frame.render_widget(format!(" {title}{}{count}", " ".repeat(pad)), head);

        let rows = self.entries.iter().map(|e| {
            let guess = match (&e.guess, e.has_squashed_subtitle()) {
                (None, _) => "(no guess)".to_string(),
                (Some(g), true) => format!("[unsplit] {}", show(&g.series, g.volume, g.subtitle.as_deref())),
                (Some(g), false) => show(&g.series, g.volume, g.subtitle.as_deref()),
            };
            let fin = e.final_fields.as_ref().map_or(String::new(), |f| {
                let name = show(&f.series, f.volume, f.subtitle.as_deref());
                if e.approved { format!("✓ {name}") } else { name }
            });
            Row::new([Cell::from(e.stem.clone()), Cell::from(guess), Cell::from(fin)])
        });
        let table = ratatui::widgets::Table::new(
            rows,
            [Constraint::Percentage(28), Constraint::Percentage(34), Constraint::Percentage(38)],
        )
        .header(Row::new(["filename", "our guess", "final name"]).style(Style::new().add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = TableState::default().with_selected(Some(self.cursor));
        frame.render_stateful_widget(table, body, &mut state);

        frame.render_widget(" ↑↓ row   ⏎ approve   e edit   ^A approve all remaining   ^S done   q quit", foot);
    }
}

/// Runs the table until the user finishes or quits.
pub fn run(table: &mut Table) -> std::io::Result<Action> {
    let mut terminal = ratatui::init();
    let result = (|| loop {
        terminal.draw(|f| table.render(f))?;
        let Event::Key(key) = crossterm::event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match table.on_key(key) {
            Action::Continue => {}
            // seam: the detail editor (next task) takes `table.entries[i]`,
            // edits `final_fields`, and returns here; until then `e` is inert.
            Action::OpenDetail(_) => {}
            done => return Ok(done),
        }
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sug(series: &str, volume: Option<u32>, subtitle: Option<&str>, source: SuggestionSource) -> Suggestion {
        Suggestion {
            series: series.into(),
            volume,
            subtitle: subtitle.map(String::from),
            volume_count: None,
            source,
        }
    }

    fn entry(stem: &str, guess: Option<Suggestion>) -> Entry {
        Entry { stem: stem.into(), guess, final_fields: None, approved: false }
    }

    fn clean(stem: &str, series: &str) -> Entry {
        entry(stem, Some(sug(series, Some(1), None, SuggestionSource::Corpus)))
    }

    fn squashed(stem: &str) -> Entry {
        entry(stem, Some(sug("Hellboy Omnibus", Some(1), Some("seedofdestruction"), SuggestionSource::Corpus)))
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn table(entries: Vec<Entry>) -> Table {
        Table::new("Humble Comics Bundle: ElfQuest".into(), entries)
    }

    fn screen(t: &Table) -> String {
        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        term.draw(|f| t.render(f)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn approve_all_remaining_leaves_already_edited_rows_alone() {
        let mut edited = clean("a", "Guessed");
        edited.final_fields = Some(ParsedTitle { series: "Typed By Hand".into(), volume: Some(7), subtitle: None });
        let mut t = table(vec![edited, clean("b", "Other")]);
        t.on_key(ctrl('a'));
        assert!(t.entries.iter().all(|e| e.approved));
        assert_eq!(t.entries[0].final_fields.as_ref().unwrap().series, "Typed By Hand");
        assert_eq!(t.entries[1].final_fields.as_ref().unwrap().series, "Other");
    }

    #[test]
    fn a_row_with_no_guess_cannot_be_approved_by_enter() {
        let mut t = table(vec![entry("nothing", None)]);
        assert_eq!(t.on_key(key(KeyCode::Enter)), Action::Continue);
        assert!(!t.entries[0].approved);
        assert!(t.entries[0].final_fields.is_none());
    }

    #[test]
    fn a_row_with_no_guess_is_not_approved_by_approve_all() {
        let mut t = table(vec![entry("nothing", None)]);
        t.on_key(ctrl('a'));
        assert!(!t.entries[0].approved);
    }

    #[test]
    fn render_shows_all_three_columns_and_the_approved_count() {
        let mut t = table(vec![clean("thecompleteelfquest_vol1", "The Complete ElfQuest"), entry("lonely", None)]);
        t.on_key(key(KeyCode::Enter));
        let s = screen(&t);
        assert!(s.contains("thecompleteelfquest_vol1"), "{s}");
        assert!(s.contains("The Complete ElfQuest Volume 1"), "{s}");
        assert!(s.contains("✓ The Complete ElfQuest Volume 1"), "{s}");
        assert!(s.contains("1 of 2 approved"), "{s}");
        assert!(s.contains("filename") && s.contains("our guess") && s.contains("final name"), "{s}");
        assert!(s.contains("(no guess)"), "{s}");
        assert!(s.contains("Humble Comics Bundle: ElfQuest"), "{s}");
    }

    #[test]
    fn approve_all_skips_a_squashed_subtitle() {
        let mut t = table(vec![clean("a", "Fine"), squashed("hellboyomnibus_seedofdestruction")]);
        t.on_key(ctrl('a'));
        assert!(t.entries[0].approved);
        assert!(!t.entries[1].approved, "a squashed subtitle must wait for the user");
        assert!(t.entries[1].final_fields.is_none());
    }

    #[test]
    fn enter_on_a_squashed_subtitle_does_not_accept_it() {
        let mut t = table(vec![squashed("x")]);
        assert_eq!(t.on_key(key(KeyCode::Enter)), Action::OpenDetail(0));
        assert!(!t.entries[0].approved);
        assert!(t.entries[0].final_fields.is_none());
    }

    #[test]
    fn a_squashed_subtitle_is_visibly_marked() {
        let s = screen(&table(vec![squashed("x"), clean("y", "Fine")]));
        assert_eq!(s.matches("[unsplit]").count(), 1, "{s}");
    }

    #[test]
    fn a_squashed_row_the_user_has_edited_can_be_approved() {
        let mut e = squashed("x");
        e.final_fields = Some(ParsedTitle {
            series: "Hellboy Omnibus".into(),
            volume: Some(1),
            subtitle: Some("Seed of Destruction".into()),
        });
        let mut t = table(vec![e]);
        t.on_key(key(KeyCode::Enter));
        assert!(t.entries[0].approved);
        assert_eq!(t.entries[0].final_fields.as_ref().unwrap().subtitle.as_deref(), Some("Seed of Destruction"));
    }

    #[test]
    fn an_override_subtitle_is_not_squashed() {
        let g = sug("S", Some(1), Some("Real Subtitle"), SuggestionSource::Override);
        assert!(!entry("x", Some(g)).has_squashed_subtitle());
    }

    #[test]
    fn enter_approves_a_clean_guess_and_toggles_it_off() {
        let mut t = table(vec![clean("a", "Fine")]);
        t.on_key(key(KeyCode::Enter));
        assert!(t.entries[0].approved);
        t.on_key(key(KeyCode::Enter));
        assert!(!t.entries[0].approved);
    }

    #[test]
    fn navigation_clamps_and_the_other_keys_report_their_action() {
        let mut t = table(vec![clean("a", "A"), clean("b", "B")]);
        t.on_key(key(KeyCode::Up));
        assert_eq!(t.cursor, 0);
        t.on_key(key(KeyCode::Down));
        t.on_key(key(KeyCode::Down));
        assert_eq!(t.cursor, 1);
        assert_eq!(t.on_key(key(KeyCode::Char('e'))), Action::OpenDetail(1));
        assert_eq!(t.on_key(ctrl('s')), Action::Done);
        assert_eq!(t.on_key(key(KeyCode::Esc)), Action::Cancel);
    }

    #[test]
    fn an_empty_table_survives_every_key() {
        let mut t = table(vec![]);
        for k in [key(KeyCode::Up), key(KeyCode::Down), key(KeyCode::Enter), key(KeyCode::Char('e')), ctrl('a')] {
            assert_eq!(t.on_key(k), Action::Continue);
        }
        assert!(screen(&t).contains("0 of 0 approved"));
    }
}
