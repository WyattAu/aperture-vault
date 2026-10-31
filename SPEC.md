# Aperture Vault: Specification
**A photo and video vault like Immich; single binary, content-addressed, no
| ML server (Python) | Face recognition + CLIP search; **on the ingest critical path** |
1. **Content-addressed storage**; every asset and every derived artifact is
2. **Embedded index**; SQLite in WAL mode instead of PostgreSQL + Redis. One
3. **Single static binary**; no Node, no Python, no Redis. One upgrade surface.
4. **ML off the critical path**; face/object models run as an optional sidecar
5. **io_uring I/O substrate**; `uring-kit` (estate L1) for bulk reads/writes
| Upload / backup clients | ✓ | ✓ | **Immich-compatible API endpoints**; existing Immich mobile apps work unchanged |
5. Enqueue derived-asset jobs (thumbnails, preview) to the worker; async.
| M0 | aperture-cas (shipped): CAS, dedup, streaming ingest, sweep; 11 tests |