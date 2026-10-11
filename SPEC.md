# Aperture Vault — Specification

**A photo and video vault like Immich — single binary, content-addressed, no
daemon circus.**

## 1. Position

Immich is the state of the art for self-hosted photo libraries. Its architecture:

| Immich component | Cost |
|---|---|
| PostgreSQL | A database server for an index that is queryable per-user and per-timeline |
| Redis | Job queue and caching between microservices |
| ML server (Python) | Face recognition + CLIP search — **on the ingest critical path** |
| microservices (server, microservices, ML, web, proxy) | Multiple processes, multiple runtimes, multiple upgrade surfaces |
| UUID-keyed filesystem blobs | Dedup only via perceptual-hash jobs *after* ingest |

Aperture's thesis: **the storage layer carries the guarantees, the services carry
none.**

1. **Content-addressed storage** — every asset and every derived artifact is
   stored under its BLAKE3 hash. Deduplication is a property of addressing
   (duplicate uploads short-circuit *during* ingest, before consuming space);
   integrity is a read-side check; thumbnails and transcodes are themselves
   content-addressed and garbage-collectable by the same rules.
2. **Embedded index** — SQLite in WAL mode instead of PostgreSQL + Redis. One
   fewer server; the timeline query is a local read; backups are one file.
3. **Single static binary** — no Node, no Python, no Redis. One upgrade surface.
4. **ML off the critical path** — face/object models run as an optional sidecar
   worker over already-stored content. Ingest never waits for inference.
5. **io_uring I/O substrate** — `uring-kit` (estate L1) for bulk reads/writes
   where available, falling back to std I/O.

## 2. Feature completeness against Immich

| Feature | Immich | Aperture plan | Mechanism |
|---|---|---|---|
| Upload / backup clients | ✓ | ✓ | **Immich-compatible API endpoints** — existing Immich mobile apps work unchanged |
| Deduplication | perceptual-hash job after ingest | ✓ by construction | BLAKE3 CAS (aperture-cas, shipped) |
| Integrity / bit-rot detection | manual, partial | ✓ | `verify`: re-hash on read, compare with address |
| EXIF / metadata | ✓ | ✓ | sidecar parse at ingest (EXIF, MP4 via estate `mp4-core`) |
| Video | ✓ | ✓ | mp4-core for probe/cover; bytes are CAS blobs |
| Live photos | ✓ | ✓ | paired-asset linkage in the index |
| RAW support | ✓ | ✓ | bytes stored; embedded preview extracted at thumbnail stage |
| Thumbnails / previews | ✓ | ✓ | derived assets are CAS blobs (Vips/jpeg-turbo or equivalent) |
| Timeline / albums | ✓ | ✓ | SQLite index |
| Multi-user + auth | ✓ | ✓ | estate `salting` + `tokenkit` (capability tokens) |
| Sharing links | ✓ | ✓ | attenuated capability tokens |
| Face / object search | ✓ | optional | sidecar ML worker (`worker-kit`), never on ingest path |
| Map view (GPS) | ✓ | ✓ | EXIF GPS in the index |
| Trash / restore | ✓ | ✓ | CAS makes delete = index flag; bytes GC'd by refcount sweep |
| Duplicated-asset detection | perceptual hash | exact + optional perceptual | exact is free (addressing); perceptual as sidecar |
| Storage template / import | ✓ | ✓ | watch folders (`worker-kit` cadence) |
| Memory view / "on this day" | ✓ | ✓ | SQLite date index |

Non-goals (stated): full-text OCR inside images at launch; editing.

## 3. Architecture

```text
┌───────────────────────────────────────────────────────┐
│  aperture-server (single binary)                      │
│    ├─ Immich-compatible API  (mobile apps, CLI)       │
│    ├─ native API             (vault-first clients)    │
│    ├─ embedded index (SQLite/WAL)                     │
│    ├─ aperture-cas (content-addressed store)  [L0]    │
│    ├─ ingest pipeline: hash-while-write → dedup →     │
│    │  probe (mp4-core) → index → enqueue derived      │
│    ├─ worker (worker-kit): thumbnails, transcodes,    │
│    │  sidecar ML drain                                │
│    ├─ auth: salting + tokenkit capability tokens      │
│    └─ resilience: breaker, healthkit, throttle-kit    │
└───────────────────────────────────────────────────────┘
        │ I/O substrate: uring-kit (Linux) / std (fallback)
        ▼
   objects/<aa>/<bb>/<blake3>   ← every byte, addressed
```

### 3.1 Ingest (the hot path)

1. Stream the upload to a staging file, hashing with BLAKE3 *while writing*
   (`aperture-cas::put_stream`). Memory is bounded by the chunk size regardless
   of asset size.
2. Address known → dedup short-circuit: drop staging, return existing.
3. Probe: mp4-core for video (duration, tracks, cover), EXIF parse for photos.
4. Index write: one SQLite transaction.
5. Enqueue derived-asset jobs (thumbnails, preview) to the worker — async.

### 3.2 What the CAS guarantees the index never has to

- No duplicate bytes: two ingests of the same file cannot occupy space twice.
- No torn assets: writes are same-directory rename (atomic on POSIX); staging
  orphans are swept by age (`sweep`), never mistaken for content.
- No silent bit-rot: `verify` re-hashes; a periodic verifier is a worker job.

## 4. Verification posture

The estate's loop (research → spec → proof) applies here too. The CAS layer's
first proofs target: atomicity (a staged rename is observable as all-or-nothing),
addressing (address = BLAKE3 of bytes), and sweeper safety (a sweep never removes
content or a live-ingest staging file). These are the spec-level invariants the
Rust test suite already pins; the Lean proofs are the next iteration's work.

## 5. Milestones

| M | Scope |
|---|---|
| M0 | aperture-cas (shipped): CAS, dedup, streaming ingest, sweep — 11 tests |
| M1 | EXIF/MP4 probe crate; SQLite index; timeline queries |
| M2 | auth (salting/tokenkit), multi-user, upload API |
| M3 | Immich-compatible endpoints (mobile apps attach) |
| M4 | thumbnails via worker-kit; storage GC |
| M5 | uring-kit substrate; sidecar ML |
