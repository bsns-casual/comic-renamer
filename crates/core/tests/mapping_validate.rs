use comic_renamer_core::mapping::{rendered_budget_ok, MappingRow};
use comic_renamer_core::preset::{komga::Komga, Preset};
use comic_renamer_core::types::Resolved;
use std::path::PathBuf;

fn row(series: &str) -> MappingRow {
    MappingRow {
        key: "k".into(),
        series: series.into(),
        volume: Some(1),
        subtitle: None,
        lang: "en".into(),
        volume_count: None,
        bundle: "humble".into(),
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

#[test]
fn validates_subtitle_and_bundle_too() {
    let mut r = row("Fine");
    r.subtitle = Some("=evil".into());
    assert!(r.validate().is_err());
    let mut r = row("Fine");
    r.bundle = "a\tb".into();
    assert!(r.validate().is_err());
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
