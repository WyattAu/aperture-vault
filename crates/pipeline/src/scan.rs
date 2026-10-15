//! Directory scan: ingest a folder tree, skipping what the vault already
//! holds. This is the watch-folder capability — point the vault at a photo
//! directory and let the dedup machinery decide what is new. The skip
//! check is the SHA-1 column in a new role: clients use it to ask "did you
//! get this already?", and the scanner asks itself the same question
//! before writing anything.

use crate::{Pipeline, PipelineError};
use aperture_cas::Store;
use aperture_index::Index;
use sha1::{Digest as _, Sha1};
use std::path::{Path, PathBuf};

/// What one scan did.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanReport {
    /// Regular files considered (hidden entries excluded).
    pub scanned: u64,
    /// Files newly ingested.
    pub ingested: u64,
    /// Files the vault already held (by SHA-1) — skipped, not re-written.
    pub skipped: u64,
    /// Files that could not be read or ingested, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// Ingest every regular, non-hidden file under `dir` (recursively). A file
/// whose SHA-1 the index already knows is skipped without a second write —
/// re-pointing the vault at the same folder is a no-op, which is the whole
/// point of content addressing.
///
/// Each candidate is read once to hash it, and — only if unknown — a second
/// time to ingest. Two sequential reads beat buffering a video in memory.
///
/// # Errors
///
/// [`PipelineError::Cas`] when the directory itself cannot be walked.
/// Per-file failures land in [`ScanReport::failed`], not here: one bad
/// file must not sink a folder.
pub fn scan_directory(
    store: &Store,
    index: &Index,
    dir: &Path,
) -> Result<ScanReport, PipelineError> {
    let mut report = ScanReport {
        scanned: 0,
        ingested: 0,
        skipped: 0,
        failed: Vec::new(),
    };
    visit(store, index, dir, &mut report)?;
    Ok(report)
}

fn visit(
    store: &Store,
    index: &Index,
    dir: &Path,
    report: &mut ScanReport,
) -> Result<(), PipelineError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        PipelineError::Cas(aperture_cas::CasError::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", dir.display()),
        )))
    })?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                report
                    .failed
                    .push((dir.to_path_buf(), format!("readdir: {e}")));
                continue;
            }
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            report
                .failed
                .push((path.clone(), "metadata unavailable".into()));
            continue;
        };
        if metadata.is_dir() {
            visit(store, index, &path, report)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        report.scanned += 1;
        match scan_one_file(index, &path) {
            Ok(true) => report.skipped += 1,
            Ok(false) => {
                let file_name = name.to_string();
                let file = match std::fs::File::open(&path) {
                    Ok(file) => file,
                    Err(e) => {
                        report.failed.push((path.clone(), format!("open: {e}")));
                        continue;
                    }
                };
                match Pipeline::ingest(store, index, file, Some(&file_name)) {
                    Ok(_) => report.ingested += 1,
                    Err(e) => report.failed.push((path.clone(), e.to_string())),
                }
            }
            Err(e) => report.failed.push((path.clone(), e)),
        }
    }
    Ok(())
}

/// `Ok(true)` when the vault already holds these bytes (skip), `Ok(false)`
/// when it does not (ingest), `Err` when the file could not be hashed.
fn scan_one_file(index: &Index, path: &Path) -> Result<bool, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha1::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|e| format!("read: {e}"))?;
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let known = index
        .asset_by_sha1(&hex)
        .map_err(|e| e.to_string())?
        .is_some();
    Ok(known)
}
