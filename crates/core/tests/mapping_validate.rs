use comic_renamer_core::mapping::{rendered_budget_ok, MappingRow};
use comic_renamer_core::preset::{komga::Komga, Preset};
use comic_renamer_core::types::Resolved;
use std::path::PathBuf;

fn row(series: &str) -> MappingRow {
    MappingRow {
        key: "k".into(),
        series: series.into(),
        volume: Some(1),
        issue: None,
        subtitle: None,
        lang: "en".into(),
        volume_count: None,
    }
}

#[test]
fn accepts_a_real_title_with_a_slash_and_a_colon() {
    assert!(row("Hack/Slash: Back to School").validate().is_ok(),
        "26.8% of real titles carry a rejected character under the old list; D47");
    assert!(row("Hypnotwist / Scarlet By Starlight").validate().is_ok());
    assert!(row("a|b \"c\" <d> e? f*\\g").validate().is_ok());
}

#[test]
fn rejects_what_sanitising_cannot_fix() {
    assert!(row("a\tb").validate().is_err(), "a tab breaks the TSV format itself");
    assert!(row("a\rb").validate().is_err());
    assert!(row("a\nb").validate().is_err());
    assert!(row("a\0b").validate().is_err());
    assert!(row("a\u{85}b").validate().is_err(), "C1 control");
    for lead in ["=cmd|'/c calc'", "+1", "-1", "@SUM(A1)"] {
        assert!(row(lead).validate().is_err(), "{lead}: contributors edit this in spreadsheets");
    }
    assert!(row("Nemesis\u{0435}").validate().is_err(), "Cyrillic e makes an invisible duplicate series");
    assert!(row("Nemesis\u{03BF}").validate().is_err(), "Greek omicron");
    assert!(row("..").validate().is_err());
    assert!(row("e\u{0301}").validate().is_err(), "non-NFC");
    assert!(row(&"x".repeat(121)).validate().is_err());
    assert!(row(&"x".repeat(120)).validate().is_ok());
}

#[test]
fn caps_bytes_not_chars() {
    assert!(row(&"é".repeat(61)).validate().is_err(), "122 bytes, 61 chars");
    assert!(row(&"é".repeat(60)).validate().is_ok());
}

#[test]
fn allows_cjk_mixed_with_latin() {
    assert!(row("ノラガミ [Noragami]").validate().is_ok());
    assert!(row("進撃の巨人 Attack on Titan").validate().is_ok());
    assert!(row("Привет мир").validate().is_ok(), "pure Cyrillic is fine");
}

/// The `bundle` half of this test went with the column (D64). Nothing stores a
/// folder name now, so there is no third field to validate — `lang` and
/// `subtitle` are what `validate` still reaches past the series.
#[test]
fn validates_subtitle_and_lang_too() {
    let mut r = row("Fine");
    r.subtitle = Some("=evil".into());
    assert!(r.validate().is_err());
    let mut r = row("Fine");
    r.lang = "a\tb".into();
    assert!(r.validate().is_err());
}

#[test]
fn rejects_an_empty_series() {
    assert!(row("").validate().is_err(), "sanitize would file it under `_`");
}

#[test]
fn rejects_a_key_that_key_would_never_produce() {
    for bad in ["Hellboy", "hell-boy", "", "hell boy"] {
        let mut r = row("Hellboy");
        r.key = bad.into();
        assert!(r.validate().is_err(), "{bad:?} can never match key(stem)");
    }
    let mut r = row("Hellboy");
    r.key = "hellboy1".into();
    assert!(r.validate().is_ok());
}

#[test]
fn rejects_cyrillic_mixed_with_greek() {
    assert!(row("\u{0435}\u{03BF}").validate().is_err());
    assert!(row("\u{3053}\u{3093}\u{3068}Latin").validate().is_ok(), "kana + Latin stays allowed");
}

#[test]
fn rejects_invisible_characters_in_every_text_field() {
    for c in ['\u{200B}', '\u{200F}', '\u{FEFF}', '\u{00AD}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}'] {
        assert!(row(&format!("Hell{c}boy")).validate().is_err(), "U+{:04X} in series", c as u32);
        let mut r = row("Fine");
        r.subtitle = Some(format!("a{c}b"));
        assert!(r.validate().is_err(), "U+{:04X} in subtitle", c as u32);
        let mut r = row("Fine");
        r.lang = format!("a{c}b");
        assert!(r.validate().is_err(), "U+{:04X} in lang", c as u32);
    }
}

/// The set the old predicate covered was the one Humble's promo prefix uses.
/// Every character here was *accepted*, and the preset passed each into the
/// path unchanged: `Hell<U+2060>boy` makes a second folder that reads as
/// `Hellboy`, which is the exact duplicate §7.3's reject column exists to stop.
#[test]
fn rejects_the_invisible_characters_the_old_set_let_through() {
    let invisible = [
        '\u{034F}', // combining grapheme joiner
        '\u{061C}', // Arabic letter mark
        '\u{115F}', // Hangul choseong filler
        '\u{1160}', // Hangul jungseong filler
        '\u{2028}', // line separator — not a control character
        '\u{2029}', // paragraph separator
        '\u{2060}', // word joiner
        '\u{2064}', // invisible plus
        '\u{206F}', // nominal digit shapes
        '\u{3164}', // Hangul filler
        '\u{FE00}', // variation selector 1
        '\u{FE0F}', // variation selector 16
        '\u{FFA0}', // halfwidth Hangul filler
        '\u{E0001}', // language tag
        '\u{E0041}', // tag letter A
    ];
    for c in invisible {
        assert!(row(&format!("Hell{c}boy")).validate().is_err(), "U+{:04X} in series", c as u32);
        let mut r = row("Fine");
        r.subtitle = Some(format!("a{c}b"));
        assert!(r.validate().is_err(), "U+{:04X} in subtitle", c as u32);
        let mut r = row("Fine");
        r.lang = format!("a{c}b");
        assert!(r.validate().is_err(), "U+{:04X} in lang", c as u32);
    }
}

/// One predicate, three callers. The validator rejected U+00AD and the bidi
/// controls where `strip_promo` cleaned neither, so a promo title carrying one
/// was partially cleaned and then refused.
#[test]
fn the_validator_and_the_preset_agree_on_what_is_invisible() {
    for c in (0u32..0x11000).chain(0xE0000..0xE0080).filter_map(char::from_u32) {
        if !comic_renamer_core::normalize::is_invisible(c) {
            continue;
        }
        let spiked = format!("Hell{c}boy");
        assert!(row(&spiked).validate().is_err(), "U+{:04X} passes the validator", c as u32);
        assert_eq!(
            comic_renamer_core::preset::komga::sanitize(&spiked),
            "Hellboy",
            "U+{:04X} survives sanitising",
            c as u32
        );
    }
}

/// `volume_count` sets the zero-padding width (D44), so an unbounded one
/// renders `Vol. 0000000005`, and a volume past the end of the run means
/// `Vol. 100` sits beside `Vol. 05` and sorts wrongly. The resolver already
/// treats volume feasibility as a hard constraint, so no corpus candidate can
/// disagree; only a hand-typed one can.
#[test]
fn bounds_volume_count_and_refuses_a_volume_past_the_end_of_the_run() {
    let with = |volume, count| {
        let mut r = row("Series");
        r.volume = volume;
        r.volume_count = count;
        r
    };
    assert!(with(Some(1), Some(9999)).validate().is_ok());
    assert!(with(Some(1), Some(10_000)).validate().is_err(), "the ceiling is 9999");
    assert!(with(Some(1), Some(4_000_000_000)).validate().is_err(), "renders Vol. 0000000001");
    assert!(with(Some(5), Some(4)).validate().is_err(), "a volume the run cannot hold");
    assert!(with(Some(4), Some(4)).validate().is_ok());
    assert!(with(Some(900), None).validate().is_ok(), "no count means the two-digit floor");
    assert!(with(None, Some(5)).validate().is_ok());
}

#[test]
fn rejects_a_whitespace_only_series() {
    for s in ["   ", " ", "\u{200B}", "\u{00A0}", "\u{3000}"] {
        assert!(row(s).validate().is_err(), "{s:?} renders as `_`");
    }
}

/// Renders the series verbatim in `.0` nested folders. Komga truncates every
/// component at 255 bytes itself, so only a preset that does not can show the
/// budget check doing work.
struct Raw(usize);
impl Preset for Raw {
    fn id(&self) -> &'static str {
        "raw"
    }
    fn destination(&self, r: &Resolved, ext: &str) -> PathBuf {
        let mut p = PathBuf::new();
        for _ in 0..self.0 {
            p.push(&r.series);
        }
        p.join(format!("leaf.{ext}"))
    }
}

struct LongLeaf;
impl Preset for LongLeaf {
    fn id(&self) -> &'static str {
        "long"
    }
    fn destination(&self, _: &Resolved, _: &str) -> PathBuf {
        PathBuf::from("s").join("y".repeat(256))
    }
}

#[test]
fn rejects_a_rendered_destination_over_budget() {
    assert!(rendered_budget_ok(&row("Fine"), &Komga));
    assert!(rendered_budget_ok(&row(&":".repeat(120)), &Komga), "worst-case expansion fits");
    assert!(rendered_budget_ok(&row(&"x".repeat(120)), &Raw(3)), "~365 bytes total");
    assert!(!rendered_budget_ok(&row(&"x".repeat(120)), &Raw(9)), "over 1024 bytes total");
    assert!(!rendered_budget_ok(&row("x"), &LongLeaf), "one 256-byte component");
}

// --- D60: the issue column's own rule ------------------------------------

#[test]
fn an_issue_past_the_ceiling_is_refused() {
    let mut r = row("Curse");
    r.issue = Some(10_000);
    assert!(r.validate().is_err(), "the ceiling is 9999");
    r.issue = Some(9_999);
    assert!(r.validate().is_ok());
    // And zero is a real issue, not absence: a `#0` ships.
    r.issue = Some(0);
    assert!(r.validate().is_ok(), "issue 0 is a value, not a missing field");
}

#[test]
fn a_row_may_carry_a_volume_and_an_issue_together() {
    // Love and Rockets Vol. IV #9: 17 rows in the Humble table are both.
    let mut r = row("Love and Rockets");
    r.volume = Some(4);
    r.issue = Some(9);
    assert!(r.validate().is_ok());
}

/// `resolved()` is the only bridge from a stored row to a rendered path, and
/// `Some(0)` is the value a filter there would silently swallow. Pinned here
/// rather than in the preset tests, which build `Resolved` directly and so
/// never cross that bridge.
#[test]
fn issue_zero_reaches_the_destination_through_resolved() {
    let mut r = row("Spawn");
    r.volume = None;
    r.issue = Some(0);
    assert_eq!(
        Komga.destination(&r.resolved(), "cbz"),
        PathBuf::from("Spawn/Spawn 0000.cbz")
    );
}
