---
document_id: YP-CAS-GC-002
version: 1.0.0
status: APPROVED (gap found, fix implemented)
domain: Storage Systems / Garbage Collection
subdomains: [Mark-Sweep, Reference Tracking, Write-Once Stores]
created: 2026-10-15
author: Loop
proof_files: [Cas.lean]
confidence_level: 0.96
tqa_level: 3
research_iterations: 3
---

# Yellow Paper: Garbage Collection — the Blob Leak the Sweep Never Covered

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| *A Study of the Deduplication File System* lineage (Data Domain, ATC 2008) | Dedup stores cannot free per-object: one chunk serves many files. GC must be whole-store, mark-sweep or refcount. |
| *Lessons from the Design of Obnam 1.x* (Wirzenius, 2014–2015 write-ups) | Obnam v1's GC (per-client checkpoint chunks) made backups progressively slower until the system collapsed. Lesson: **GC design is chosen at birth**, and "delete = drop a reference" must have a story from day one. |
| git's reachability GC (gc.mark-and-sweep with grace periods) | Two-phase GC with an expiry grace period is simple, safe, and runs incrementally — the proven shape for content-addressed stores. |
| `cargo vet` / crate ecosystems aside — the CAS's own horizon sweeper (M0) | Sweeps *staging files* by age horizon. It never touches committed objects. Correct for its job — and never a GC. |

## YP-2: The gap the research found

`trash_empty` removes index rows, memberships, and derived mappings —
and then **the CAS blobs stay forever**. The M0 sweeper only collects
abandoned staging files; nothing in the system ever unlinks a committed
object. The vault leaks every deleted photo's bytes until the disk is
full. Found by iteration 2 of this loop, by walking "what happens to
the bytes" after the delete flow — the index-centric view had hidden it.

## YP-3: How the field solves it — and what we adopt

| Strategy | Used by | Verdict for the vault |
|---|---|---|
| Per-chunk refcounts | Data Domain (in index) | Refcounts break on every aliasing bug; obnam lesson. Refused. |
| Mark-sweep over index reachability | git, restic's `prune` | **Adopted.** The index is the reachability root set (assets ∪ derived). Mark from it, sweep the objects dir. |
| Grace period before sweep | git (default 2 weeks) | Adopted in spirit: `gc` is explicit (`aperture-server gc`), never automatic — an operator runs it; no race with a concurrent ingest that has not yet committed its index row. |
| Chunk-index incremental GC | Data Domain | Refused: no chunking (YP-CAS-CHUNKING-001). |

## YP-4: Spec consequence — the invariant

**GC safety:** after a sweep, every address the index holds resolves to
bytes on disk. Stated as `gc_safety` in `proofs/Cas.lean` and
machine-checked over the filter model: the sweep keeps exactly the
marked objects. The implementation (`Pipeline::gc`) must satisfy: mark
= every `assets.address` ∪ every `derived.address`; sweep = unlink
objects absent from the mark set; the sweep is explicit and operator-run.

## YP-5: Implemented

`Pipeline::gc(store, index) -> GcReport` — mark-sweep over the objects
root, report of removed bytes/objects, unit-tested with an orphaned
blob surviving an ingest + dying at gc, and a referenced blob surviving
gc after trash-empty of its owner.
