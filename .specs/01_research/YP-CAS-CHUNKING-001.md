---
document_id: YP-CAS-CHUNKING-001
version: 1.0.0
status: APPROVED (refusal)
domain: Storage Systems / Deduplication
subdomains: [Content-Defined Chunking, Whole-File Dedup, Photo Workloads]
created: 2026-10-15
author: Loop
proof_files: [Cas.lean]
confidence_level: 0.95
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: Content-Defined Chunking — Refused for a Photo Workload

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| Venti: *A New Approach to Archival Storage* (Quinlan & Dorward, USENIX FAST 2002) | The founding content-addressed archive: whole-file hashing, write-once, dedup by address. Exactly the vault's model. |
| *Avoiding the Disk Bottleneck in the Data Domain Deduplication Filesystem* (Zhu, Li & Patterson, USENIX ATC 2008) | Chunk-level dedup at backup scale: variable-size chunks, sparse index ("buddy" localities), stream-informed cache. The chunking workhorse paper. |
| *FastCDC: A Fast and Efficient Content-Defined Chunking Approach* (Wia~Xia et al., USENIX ATC 2016) | Cut-Point skipping + normalized hashing: CDC fast enough for production; Gear hashing follow-up (2017) makes it branch-free. |
| restic design document; BorgBackup internals; Kopia docs | The three open-source backup giants all run CDC (Rabin/Gear variants) because backup workloads are *delta-heavy*: month-old VM image + today's VM image share 99% of bytes. |
| Immich / PhotoPrism architecture docs | Neither deduplicates at all: one upload = one stored copy, ever. The vault's whole-file CAS already exceeds the photo-server state of the art. |

## YP-2: The question

Should the CAS chunk files (CDC) so that two videos sharing footage, or
a re-exported JPEG with a new tag, deduplicate partially?

## YP-3: Analysis — the workload decides, not the fashion

CDC wins when a large file changes by a small delta. That describes:
VM images, disk backups, log files, office documents. It does **not**
describe a personal photo library:

- A photo is immutable. A re-edited photo is a *different* photo; its
  new bytes deserve their own object. Chunk-level overlap between two
  edits of one JPEG is negligible because JPEG re-encoding avalanches
  the entropy stream — CDC finds almost no shared chunks (the delta
  assumption fails at the codec level, not the tooling level).
- Chunking a media library multiplies object count by ~10³ and turns
  the index into the bottleneck Data Domain had to solve with a sparse
  index and a stream cache — machinery a single-user vault does not
  need.
- The one real CDC win for this workload — "video A and video B share
  40 minutes of footage" — is a *library curation* concern (Immich
  solves it socially: do not import two copies), not a storage
  concern.

## YP-4: Comparison verdict

| System | Choice | Why theirs | Why ours |
|---|---|---|---|
| restic / borg / kopia | CDC | Backup deltas | N/A — not backups |
| Immich / PhotoPrism | none | Simplicity | Whole-file CAS beats them with no cost |
| Venti | whole-file (archives) | Immutability | Same workload, same answer |
| **aperture-vault** | **whole-file** | Photos are immutable | — |

## YP-5: Spec consequence

Whole-file dedup is the documented, permanent choice for photo and
video bytes. CDC is not deferred, it is **refused**: adding it would
multiply objects and complexity for a workload whose deltas do not
exist. The machine-checked core of the claim (dedup by construction)
lives in `proofs/Cas.lean::ingest_is_idempotent`.

If a future feature ingests delta-heavy files (document archives), it
must come back to this paper first and re-derive the workload argument
in writing.
