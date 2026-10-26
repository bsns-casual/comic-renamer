//! Task 1 of the mapping-first rebuild. The key rule is frozen: every row ever
//! contributed was keyed by it.

use comic_renamer_core::mapping::{key, parse_tsv, to_tsv, MappingRow};
use comic_renamer_core::types::Status;
use std::collections::HashMap;

/// Test-only reader for the fixtures, which are a different format from the
/// mapping TSV. Returns `(title, stem)`; rows with no stem field are skipped.
fn fixture(text: &str, stem_col: usize) -> Vec<(String, String)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let stem = f.get(stem_col)?;
            if stem.is_empty() {
                return None;
            }
            Some((f[1].to_string(), stem.to_string()))
        })
        .collect()
}

fn humble() -> Vec<(String, String)> {
    fixture(include_str!("../../../fixtures/humble-ground-truth.tsv"), 2)
}

fn marketplace() -> Vec<(String, String)> {
    fixture(include_str!("../../../fixtures/marketplace-ground-truth.tsv"), 2)
}

fn row() -> MappingRow {
    MappingRow {
        key: "hackslashbacktoschool".into(),
        series: "Hack/Slash: Back to School".into(),
        volume: Some(3),
        issue: None,
        subtitle: Some("A: B".into()),
        lang: "en".into(),
        volume_count: Some(12),
        bundle: "Some Bundle".into(),
    }
}

#[test]
fn key_is_frozen_at_2036_over_the_humble_fixture() {
    let rows = humble();
    assert_eq!(rows.len(), 2223, "fixture reader must skip only the stemless line");
    let keys: std::collections::HashSet<String> =
        rows.iter().filter_map(|(_, s)| key(s)).collect();
    assert_eq!(keys.len(), 2036, "the key rule is frozen; see D40");
}

#[test]
fn key_collides_for_no_two_distinct_titles() {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut all = humble();
    assert_eq!(marketplace().len(), 12);
    all.extend(marketplace());
    for (title, stem) in all {
        let Some(k) = key(&stem) else { continue };
        if let Some(prev) = seen.insert(k.clone(), title.clone()) {
            assert_eq!(prev, title, "key {k:?} maps two different titles");
        }
    }
}

#[test]
fn key_is_none_when_nothing_alphanumeric_survives() {
    assert_eq!(key("___"), None);
    assert_eq!(key("()"), None);
    assert_eq!(key(" "), None);
    assert_eq!(key(""), None);
    assert!(key("a").is_some());
}

#[test]
fn key_is_nfc_lowercase_alphanumeric_only() {
    // NFD "e" + combining acute must equal precomposed "é".
    assert_eq!(key("Cafe\u{301}"), key("Caf\u{e9}"));
    assert_eq!(key("Caf\u{e9}"), Some("caf\u{e9}".into()));
    assert_eq!(key("Hack/Slash: Vol_1 (2020)"), Some("hackslashvol12020".into()));
    // Digits-only survives: "(1)" is key "1", not an empty key.
    assert_eq!(key("(1)"), Some("1".into()));
}

#[test]
fn parse_tsv_strips_crlf_and_a_bom() {
    let rows = parse_tsv("\u{feff}key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\tbundle\r\nx\tX\t\t\t\ten\t\tb\r\n").unwrap();
    assert_eq!(rows[0].key, "x");
    assert_eq!(rows[0].series, "X");
    assert_eq!(rows[0].volume, None);
    assert_eq!(rows[0].issue, None);
    assert_eq!(rows[0].subtitle, None);
    assert_eq!(rows[0].bundle, "b", "a trailing CR must not survive in the last field");
}

#[test]
fn parse_tsv_errors_carry_the_line_number() {
    let hdr = "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\tbundle\n";
    let bad_header = parse_tsv("key\tseries\n").unwrap_err().to_string();
    assert!(bad_header.contains("line 1"), "{bad_header}");
    let short = parse_tsv(&format!("{hdr}x\tX\n")).unwrap_err().to_string();
    assert!(short.contains("line 2"), "{short}");
    let nan = parse_tsv(&format!("{hdr}x\tX\tabc\t\t\ten\t\tb\n")).unwrap_err().to_string();
    assert!(nan.contains("line 2"), "{nan}");
    // A non-numeric issue is named by its own column, not the volume's.
    let bad_issue = parse_tsv(&format!("{hdr}x\tX\t\tabc\t\ten\t\tb\n")).unwrap_err().to_string();
    assert!(bad_issue.contains("line 2") && bad_issue.contains("issue"), "{bad_issue}");
}

#[test]
fn tsv_round_trips_a_title_with_a_slash_and_a_colon() {
    let r = row();
    assert_eq!(parse_tsv(&to_tsv(&[r.clone()])).unwrap()[0], r);
    let none = MappingRow { volume: None, subtitle: None, volume_count: None, ..row() };
    assert_eq!(parse_tsv(&to_tsv(&[none.clone()])).unwrap()[0], none);
}

#[test]
fn resolved_carries_the_row_into_the_preset_unchanged() {
    let r = row().resolved();
    assert_eq!(r.series, "Hack/Slash: Back to School");
    assert_eq!(r.volume, Some(3));
    assert_eq!(r.subtitle.as_deref(), Some("A: B"));
    assert_eq!(r.run, 12);
    assert_eq!(r.status, Status::Mapped);
    assert!(r.record.is_none());
    let none = MappingRow { volume_count: None, ..row() }.resolved();
    assert_eq!(none.run, 0);
}

// --- D60: an issue is its own column -------------------------------------

#[test]
fn a_row_with_an_issue_round_trips_through_the_tsv() {
    let row = MappingRow {
        key: "beeandpuppycatissue1".into(),
        series: "Bee and PuppyCat".into(),
        volume: None,
        issue: Some(1),
        subtitle: None,
        lang: "en".into(),
        volume_count: None,
        bundle: "Humble BOOM! Bundle".into(),
    };
    assert_eq!(parse_tsv(&to_tsv(&[row.clone()])).unwrap(), vec![row]);
}

#[test]
fn a_seven_column_file_is_refused_by_name() {
    // No release has shipped, so there is no migration — but a stale file must
    // say what is wrong with it rather than drop rows.
    let old = "key\tseries\tvolume\tsubtitle\tlang\tvolume_count\tbundle\nk\tS\t\t\ten\t\tB\n";
    let err = parse_tsv(old).unwrap_err();
    assert!(
        format!("{err:?}").contains("issue"),
        "the error must name the expected header so a stale file is legible: {err:?}"
    );
}
