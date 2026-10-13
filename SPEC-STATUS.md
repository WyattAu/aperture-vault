# Status — after the first estate-dogfooding arc (2026-10-12)

## Shipped

- **M0** `aperture-cas` — BLAKE3-addressed store: streaming ingest (bounded
  memory, hashed while written), atomic same-directory rename, dedup by
  construction, corruption detection, sharded layout, horizon sweeper.
- **M1** `aperture-probe` — JPEG/EXIF (both endiannesses, GPS DMS→decimal),
  PNG IHDR dimensions, MP4 duration/dimensions via estate mp4-core. Total
  parsing: corrupt bytes yield an empty record, never a failed listing.
- **M2** `aperture-index` — SQLite/WAL asset index keyed by content address;
  timeline newest-first by capture day, sentinel-day grouping for untagged
  assets, stable pagination. `aperture-auth` — salted Argon2 verifiers
  (estate salting), session JWTs, share-link capability tokens (estate
  tokenkit), account-enumeration defence, minimum-length password policy.
  `aperture-pipeline` — the ingest junction: hash-while-stream → dedup
  short-circuit → probe what survived → index what probed.

## Dogfood used

`mp4-core` (video probe; contributed `read_video_dimensions_in` upstream),
estate CI gate, `salting` + `tokenkit` from crates.io (the same pins
`estate-integration` verifies).

## Honest open items

1. **CI fails at the private-sibling checkout** until an `ESTATE_TOKEN`
   secret (a PAT with estate read access) exists — the workflow documents
   this at the failing step. mp4-core, id3-core, audiobook-core and
   audiobook-conformance are private; a public vault whose dependencies are
   private repos is one nobody else can build, which is a visibility
   decision above this repository.
2. **Auth is session+share only** — no OAuth, no email flows. The JWT
   substrate makes adding them mechanical.
3. **Thumbnails/transcodes** (M4) are unimplemented; derived assets are
   already addressable CAS blobs, so the storage needs no migration.
4. **uring-kit** substrate (M5) unimplemented; std I/O today.
