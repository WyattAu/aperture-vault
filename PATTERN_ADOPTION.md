# Pattern adoption: aperture-vault

Relationships to the pattern canon (pattern-standards), verified against code.
Format: pattern-standards/references/adoption-format.md.

| Pattern | Status | Evidence |
|---|---|---|
| T1 | adopted | crates/probe/src/lib.rs:55 (raw bytes parsed into typed AssetMeta once at ingest; callers never re-validate) |
| T13 | adopted | crates/pipeline/src/scan.rs:40, crates/server/src/main.rs:145 (scan, thumbnail, and phash are pure pipeline functions; the axum router is the shell) |
| T28 | adopted | crates/cas/src/lib.rs:1 (BLAKE3 content-addressed blob store: dedup, integrity verification, and GC all follow from addressing) |
