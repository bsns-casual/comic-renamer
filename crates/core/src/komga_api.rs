//! Komga pre-flight. Hashing enabled *after* a rename cannot help: a deleted
//! record can never acquire a hash (D35).

use crate::types::{KomgaError, KomgaLibrary};

/// Pure, so it is testable without a server. Requires `hashFiles == true` and
/// `emptyTrashAfterScan == false`, and refuses otherwise.
pub fn assert_safe(libraries: &[KomgaLibrary]) -> Result<(), KomgaError> {
    todo!()
}

/// The token comes from `COMIC_RENAMER_KOMGA_TOKEN`, never a flag — flags land
/// in shell history.
pub fn check_libraries(url: &str) -> Result<Vec<KomgaLibrary>, KomgaError> {
    todo!()
}
