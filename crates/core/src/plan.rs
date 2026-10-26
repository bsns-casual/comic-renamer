//! JSON Lines: header object, then one row per line (§6.9). TSV is ruled out —
//! tabs occur in titles and a leading `=` is formula injection.

use crate::corpus::sha256_hex;
use crate::types::{PlanError, PlanHeader, PlanRow};
use std::path::{Component, Path};

/// The plan format this build writes and reads. v2: statuses are
/// `mapped`/`unmapped`; a v1 plan carries resolver statuses `apply` no longer
/// understands. v3: rows carry `issue` (D60); a v2 row has none, and `apply`
/// recomputes the destination from the row's fields, so reading one would
/// place a single issue under a volume-less, issue-less name.
pub const PLAN_VERSION: u32 = 3;

pub fn write(header: &PlanHeader, rows: &[PlanRow], path: &Path) -> Result<(), PlanError> {
    let stamped = PlanHeader {
        checksum: checksum(header, rows),
        ..header.clone()
    };
    let mut out = serde_json::to_string(&stamped).map_err(parse_error(0))?;
    for (i, row) in rows.iter().enumerate() {
        out.push('\n');
        out.push_str(&serde_json::to_string(row).map_err(parse_error(i + 1))?);
    }
    out.push('\n');
    std::fs::write(path, out)?;
    Ok(())
}

/// Validates the whole file and **never** returns a partial row set: one bad
/// row rejects everything. An edited plan is reported as edited, not rejected —
/// `apply` revalidates rather than trusting it.
///
/// "Reported" means `ChecksumMismatch` specifically: the variant exists so a
/// caller can tell a human's edit apart from a malformed file and decide. Every
/// other failure here is fatal.
pub fn read(path: &Path) -> Result<(PlanHeader, Vec<PlanRow>), PlanError> {
    let (header, rows, edited) = read_allowing_edits(path)?;
    if edited {
        let actual = checksum(&header, &rows);
        return Err(PlanError::ChecksumMismatch {
            expected: header.checksum,
            actual,
        });
    }
    Ok((header, rows))
}

/// As `read`, but a stale checksum is **reported** rather than fatal: the
/// third element is true when a human has edited the file.
///
/// §6.9 says an edited plan is revalidated, not rejected, which `read` alone
/// cannot express — it has no way to hand back the rows it refused. `apply`
/// is the caller that needs them; every other validation is still fatal here.
pub fn read_allowing_edits(path: &Path) -> Result<(PlanHeader, Vec<PlanRow>, bool), PlanError> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());

    let header_line = lines.next().ok_or(PlanError::MissingHeader)?;
    // The version comes out of the raw JSON first. Deserialising the whole
    // header would fail on a field an old plan lacks, and an old row's status
    // would fail with an unknown-variant message: both hide the real reason,
    // which is "scan again".
    #[derive(serde::Deserialize)]
    struct Versioned {
        version: u32,
    }
    let Versioned { version } = serde_json::from_str(header_line).map_err(parse_error(1))?;
    if version != PLAN_VERSION {
        return Err(PlanError::UnsupportedVersion {
            found: version,
            expected: PLAN_VERSION,
        });
    }
    let header: PlanHeader = serde_json::from_str(header_line).map_err(parse_error(1))?;

    let mut rows: Vec<PlanRow> = Vec::new();
    for (offset, line) in lines.enumerate() {
        let row: PlanRow = serde_json::from_str(line).map_err(parse_error(offset + 2))?;

        if rows.iter().any(|existing| existing.id == row.id) {
            return Err(PlanError::DuplicateRowId(row.id));
        }
        // Lexical containment only: real containment is enforced per component
        // at the syscall (§6.11). This catches a hand-edited plan before any
        // of it reaches the filesystem.
        if escapes(&row.destination_preview) {
            return Err(PlanError::OutsideDestRoot(row.destination_preview));
        }
        rows.push(row);
    }

    let edited = checksum(&header, &rows) != header.checksum;
    Ok((header, rows, edited))
}

/// `destination_preview` is relative to `dest_root`. Anything absolute, or
/// anything that walks upward, leaves the tree.
fn escapes(destination: &Path) -> bool {
    destination.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// Over the header and every row, so `apply` can tell what a human changed.
///
/// The header's own `checksum` is excluded — including it would make the value
/// self-referential and no plan could ever verify.
pub fn checksum(header: &PlanHeader, rows: &[PlanRow]) -> String {
    let bare = PlanHeader {
        checksum: String::new(),
        ..header.clone()
    };
    let mut material = serde_json::to_string(&bare).unwrap_or_default();
    for row in rows {
        material.push('\n');
        material.push_str(&serde_json::to_string(row).unwrap_or_default());
    }
    sha256_hex(material.as_bytes())
}

fn parse_error(line: usize) -> impl Fn(serde_json::Error) -> PlanError {
    move |e| PlanError::Parse {
        line,
        message: e.to_string(),
    }
}
