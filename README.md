# Aperture Vault

**A photo and video vault; single binary, content-addressed, Immich-compatible
API.** Every asset lives under its own BLAKE3 hash, so deduplication, integrity
and garbage collection are properties of the storage, not features of a pipeline.

See [SPEC.md](SPEC.md) for the full Immich comparison and architecture.

## Status

M0 shipped: `aperture-cas`, the content-addressed store; atomic streaming
ingest, deduplication by construction, corruption detection, orphan sweeping.
11 tests.

## CAS quick look

```rust
use aperture_cas::Store;

let store = Store::open("vault")?;

// Stream an upload: hashed while written, deduplicated on hit.
let ingested = store.put_stream(file)?;
println!("{} ({} bytes, new: {})", ingested.address, ingested.len, ingested.newly_written);

// Read back; verify detects bit-rot by re-hashing.
let bytes = store.get_bytes(&ingested.address)?;
assert!(store.verify(&ingested.address)? == Some(true));
```

## Layout

- `crates/cas`; content-addressed store (L0): BLAKE3 addressing, atomic
  same-directory rename ingest, sharded object layout, orphan sweeper.
