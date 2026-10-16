//! Garbage collection: mark-sweep over the objects root.
//!
//! YP-CAS-GC-002 found the gap — `trash_empty` removed index rows while
//! the CAS blobs stayed forever; the M0 horizon sweeper only collects
//! abandoned staging files. The fix is the field's standard shape (git's
//! prune, restic's prune): mark from the index's reachability roots, sweep
//! the objects root of everything unmarked, run it explicitly.
//!
//! The sweep is deliberately **operator-run, never automatic**: an
//! automatic pass could race an ingest whose index row is not yet written
//! and collect live bytes. `gc_safety` in the proof suite
//! (`.specs/02_architecture/proofs/Cas.lean`) is the rule this
//! implementation must satisfy: sweeping keeps every marked object.

use crate::PipelineError;
use aperture_cas::Store;
use aperture_index::Index;

/// What one garbage-collection pass did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcReport {
    /// Objects removed from the objects root.
    pub removed: u64,
    /// Bytes reclaimed.
    pub bytes_reclaimed: u64,
    /// Objects kept (the mark set size, for the report).
    pub kept: u64,
}

/// Mark from the index (live assets ∪ derived mappings), sweep the objects
/// root of everything unmarked.
///
/// Callers must guarantee no ingest is mid-flight: the pass is
/// operator-run (`aperture-server gc`), single-user, and the ingest path
/// writes its index row after the CAS object — an unrecorded object is
/// indistinguishable from garbage for exactly that window.
///
/// # Errors
///
/// [`PipelineError::Cas`] when the objects root cannot be read or an
/// object cannot be unlinked; [`PipelineError::Index`] when the mark set
/// cannot be read.
pub fn gc(store: &Store, index: &Index) -> Result<GcReport, PipelineError> {
    let marked = index.mark_set().map_err(PipelineError::Index)?;
    let mut report = GcReport {
        kept: marked.len() as u64,
        ..GcReport::default()
    };
    let objects_root = store.root().join("objects");
    sweep_dir(&marked, &objects_root, &mut report)?;
    Ok(report)
}

fn sweep_dir(
    marked: &std::collections::HashSet<String>,
    dir: &std::path::Path,
    report: &mut GcReport,
) -> Result<(), PipelineError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        PipelineError::Cas(aperture_cas::CasError::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", dir.display()),
        )))
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            PipelineError::Cas(aperture_cas::CasError::Io(std::io::Error::new(
                e.kind(),
                format!("readdir: {e}"),
            )))
        })?;
        let path = entry.path();
        if path.is_dir() {
            sweep_dir(marked, &path, report)?;
            // A shard directory emptied by the sweep is removed too, so
            // the tree collapses back as bytes go away.
            let empty = std::fs::read_dir(&path)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(&path);
            }
            continue;
        }
        // The object file name is the hex address (the CAS layout stores
        // each blob as <hex>); recover it without re-reading bytes.
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if marked.contains(file_name) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        std::fs::remove_file(&path).map_err(|e| {
            PipelineError::Cas(aperture_cas::CasError::Io(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", path.display()),
            )))
        })?;
        report.removed += 1;
        report.bytes_reclaimed += size;
    }
    Ok(())
}
