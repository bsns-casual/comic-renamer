//! Komga pre-flight. Hashing enabled *after* a rename cannot help: a deleted
//! record can never acquire a hash (D35).

use crate::types::{KomgaError, KomgaLibrary};

/// Pure, so it is testable without a server. Requires `hashFiles == true` and
/// `emptyTrashAfterScan == false`, and refuses otherwise.
///
/// §6.8.2: a rename is a delete-plus-add, and Komga rematches by size then
/// hash. `tryRestoreBooks` only considers deleted records that already carry
/// a hash, and switching hashing on afterwards hashes books by reading their
/// *current* path — which after a rename no longer exists. So the setting is
/// worthless unless it was already on. `emptyTrashAfterScan` is the second
/// trap: it purges deleted records immediately, defeating restore whatever
/// the hash setting.
///
/// Both default the safe way, so the common case passes. A user who changed
/// either loses read progress silently, and silence is the problem — refuse
/// rather than warn.
pub fn assert_safe(libraries: &[KomgaLibrary]) -> Result<(), KomgaError> {
    for library in libraries {
        if !library.hash_files || library.empty_trash_after_scan {
            return Err(KomgaError::Unsafe {
                name: library.name.clone(),
                hash_files: library.hash_files,
                empty_trash: library.empty_trash_after_scan,
            });
        }
    }
    Ok(())
}

/// The token comes from `COMIC_RENAMER_KOMGA_TOKEN`, never a flag — flags land
/// in shell history.
pub fn check_libraries(url: &str) -> Result<Vec<KomgaLibrary>, KomgaError> {
    let token = std::env::var("COMIC_RENAMER_KOMGA_TOKEN")
        .map_err(|_| KomgaError::MissingToken)?;
    let endpoint = format!("{}/api/v1/libraries", url.trim_end_matches('/'));

    let mut response = ureq::get(&endpoint)
        .header("X-API-Key", &token)
        .call()
        .map_err(|e| KomgaError::Http(e.to_string()))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| KomgaError::Http(e.to_string()))?;

    parse_libraries(&body)
}

/// Split out so the shape of Komga's response is testable without a server.
pub fn parse_libraries(body: &str) -> Result<Vec<KomgaLibrary>, KomgaError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Wire {
        name: String,
        // Absent on older Komga builds. Defaulting these to the *unsafe*
        // value would refuse every such server; defaulting to the safe one
        // would wave it through. Neither is right, so they are Options and a
        // missing field is reported as the refusal it is.
        hash_files: Option<bool>,
        empty_trash_after_scan: Option<bool>,
    }

    let wire: Vec<Wire> =
        serde_json::from_str(body).map_err(|e| KomgaError::Http(e.to_string()))?;
    wire.into_iter()
        .map(|w| match (w.hash_files, w.empty_trash_after_scan) {
            (Some(hash_files), Some(empty_trash_after_scan)) => Ok(KomgaLibrary {
                name: w.name,
                hash_files,
                empty_trash_after_scan,
            }),
            // Reported, never guessed: this server cannot be shown to be safe.
            _ => Err(KomgaError::Http(format!(
                "library {:?} did not report hashFiles and emptyTrashAfterScan",
                w.name
            ))),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(hash_files: bool, empty_trash_after_scan: bool) -> KomgaLibrary {
        KomgaLibrary {
            name: "Comics".into(),
            hash_files,
            empty_trash_after_scan,
        }
    }

    #[test]
    fn assert_safe_refuses_both_unsafe_settings() {
        assert!(assert_safe(&[lib(false, false)]).is_err());
        assert!(assert_safe(&[lib(true, true)]).is_err());
        assert!(assert_safe(&[lib(false, true)]).is_err());
        assert!(assert_safe(&[lib(true, false)]).is_ok());
    }

    #[test]
    fn one_unsafe_library_refuses_the_whole_check() {
        // The user may be renaming into any of them, and the plan does not
        // say which. Refusing only the named one would be a guess.
        let libraries = [lib(true, false), lib(false, false), lib(true, false)];
        assert!(assert_safe(&libraries).is_err());
    }

    #[test]
    fn no_libraries_is_not_an_error() {
        // Nothing to lose read progress in.
        assert!(assert_safe(&[]).is_ok());
    }

    #[test]
    fn parses_komgas_shape() {
        let body = r#"[{"name":"Comics","hashFiles":true,"emptyTrashAfterScan":false}]"#;
        let parsed = parse_libraries(body).unwrap();
        assert_eq!(parsed, vec![lib(true, false)]);
        assert!(assert_safe(&parsed).is_ok());
    }

    #[test]
    fn a_server_that_omits_the_settings_is_refused_not_assumed() {
        // Defaulting to the safe value waves through a server that cannot be
        // shown to be safe; defaulting to the unsafe one refuses every older
        // Komga. Report it instead.
        let body = r#"[{"name":"Comics","hashFiles":true}]"#;
        assert!(parse_libraries(body).is_err());
    }
}
