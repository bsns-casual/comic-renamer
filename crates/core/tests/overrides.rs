//! Task 4. Layer B is contributed data from strangers: §6.11's forbid list is
//! the whole point of the file, so most of this is rejection.

use comic_renamer_core::overrides::Overrides;
use std::path::PathBuf;
use tempfile::TempDir;

fn write(contents: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("overrides.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

/// TOML basic strings escape only `\\` and `"`; Rust's `escape_default`
/// would emit `\\'`, which TOML rejects, and the test would then pass for
/// the wrong reason.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn override_with_series(series: &str) -> (TempDir, PathBuf) {
    write(&format!(
        "[\"SOME_Key\"]\nseries = \"{}\"\nbundle = \"Some Bundle\"\n",
        toml_escape(series)
    ))
}

fn override_without_bundle() -> (TempDir, PathBuf) {
    write("[\"SOME_Key\"]\nseries = \"Some Series\"\n")
}

fn twd_override() -> (TempDir, PathBuf) {
    write(
        "[\"TWD_CompendiumOne\"]\nseries = \"The Walking Dead\"\nvolume = 1\nbundle = \"Image Comics in the '10s\"\n",
    )
}

#[test]
fn rejects_path_shaped_and_invisible_fields() {
    for bad in [
        "../etc",
        "a/b",
        r"C:\x",
        r"\\srv\s",
        "/abs",
        "CON",
        "CON.pdf",
        "x.",
        "x ",
        "=cmd()",
        "Hell\u{200b}boy",
        "\u{202e}yobleH",
    ] {
        let (_d, path) = override_with_series(bad);
        assert!(Overrides::load(&path).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn rejects_a_cyrillic_lookalike() {
    // §6.11: a Cyrillic 'е' makes a duplicate series invisible in a diff.
    let (_d, path) = override_with_series("H\u{0435}llboy");
    assert!(Overrides::load(&path).is_err());
}

#[test]
fn accepts_a_title_that_only_looks_hostile() {
    // Apostrophes and ampersands are legal in a title and must survive, or
    // the forbid list quietly becomes a corpus filter.
    let (_d, path) = override_with_series("Bourdain's Hack & Slash");
    assert!(Overrides::load(&path).is_ok());
}

#[test]
fn requires_a_bundle_discriminator() {
    let (_d, path) = override_without_bundle();
    assert!(Overrides::load(&path).is_err());
}

#[test]
fn bundle_matches_as_squashed_substring_of_the_folder() {
    let (_d, path) = twd_override();
    let o = Overrides::load(&path).unwrap();
    assert!(o.lookup("TWD_CompendiumOne", "HumbleImageComicsInThe10s").is_some());
    assert!(o.lookup("TWD_CompendiumOne", "HumbleBoomBundle").is_none());
}

#[test]
fn caps_field_length() {
    let (_d, path) = override_with_series(&"x".repeat(121));
    assert!(Overrides::load(&path).is_err());
}

#[test]
fn the_shipped_overrides_file_loads() {
    // data/overrides.toml is contributed data held to its own forbid list;
    // nothing else proves the shipped file passes it.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/overrides.toml");
    let loaded = Overrides::load(&path);
    assert!(loaded.is_ok(), "{:?}", loaded.err());
    let o = loaded.unwrap();
    assert!(o
        .lookup("TWD_CompendiumOne", "HumbleImageComicsBundle2ImageFirsts")
        .is_some());
    assert!(o.lookup("SOA_04", "HumbleBOOMBundle").is_some());
}
