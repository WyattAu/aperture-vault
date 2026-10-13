# Status — after the Immich-API milestone (2026-10-13)

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
- **M3** Immich-compatible API in `aperture-server` — the protocol the
  existing Immich mobile apps speak, so clients attach unchanged:
  `POST /api/auth/login` (`{email, password}` → `{accessToken, userId}`,
  `username` accepted as an alias — one login endpoint serves both client
  generations), `POST /api/auth/validateToken`, `GET /api/users/me` (full
  profile via the new `UserStore::user_by_id`), multipart `assetData`
  upload, and `GET /api/assets/{id}/original`. One upload URL serves both
  encodings: a dispatcher branches on Content-Type, handing multipart
  bodies to the Immich path and raw bytes + `?filename=` to the native
  path, so each client keeps its exact contract at the same endpoint.
  Asset ids are content addresses, not UUIDs — clients treat ids as
  opaque strings, and an id that lies about identity would be a defect.

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
