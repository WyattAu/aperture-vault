# Status: after the ThumbHash milestone (2026-10-14)
- **M0** `aperture-cas`; BLAKE3-addressed store: streaming ingest (bounded
- **M1** `aperture-probe`; JPEG/EXIF (both endiannesses, GPS DMS→decimal),
- **M2** `aperture-index`; SQLite/WAL asset index keyed by content address;
  assets, stable pagination. `aperture-auth`; salted Argon2 verifiers
  `aperture-pipeline`; the ingest junction: hash-while-stream → dedup
- **M3** Immich-compatible API in `aperture-server`; the protocol the
  `username` accepted as an alias; one login endpoint serves both client
  Asset ids are content addresses, not UUIDs; clients treat ids as
  **synchronous on-demand with a deterministic cache**; a deliberate
- **M5; planned as uring-kit under the CAS, refuted by measurement, pivoted
  removes syscall *entry* cost; ~2 µs per op, ~6 ops per ingest; so its
  userspace to hash them. uring-kit stays what it is; a socket substrate
  version`; the connect-time handshake, every feature flag stating only
  bulk-upload-check`; the call that makes backup resumable: the phone
  returns Immich's `{id, status: created|duplicate}`; `duplicate` is
- **Albums**; the app's album screens, grounded in the same OpenAPI spec:
  tests pin: an album is a *view* over assets; it never gains phantom
  dogfooding found missing; the config endpoint promised it), connect
- **ThumbHash placeholders**; the last visual gap. One decode of the
  clients generate. `thumbhash = "0.1"` is evanw's own algorithm; pure
- **Share links**; the auth layer's tokenkit capability was minted and
  /api/shared-links/me?key=` for the visitor; no session, album plus
  of truth; a key opens assets only while its row exists, and asset
- **Asset detail**; `GET /api/assets/{id}`, the record the app opens per
  bounded 16-byte read; never the whole blob for a content type. The
- **Memories ("on this day")**; the feature that looks like ML and is
  assets without capture dates are never memories; a guess would be a
- **Favorite + statistics**; `PUT /api/assets/{id}` with the honest slice
  refused by omission; accepted-and-ignored would lie), and `GET
- **Search**; `POST /api/search/metadata` over the fields the vault
  assembled from numbered placeholders only; user input is always a
- **Trash**; `DELETE /api/assets` (trash; `force` skips to permanent),
  every read; timeline, buckets, search, memories, statistics, albums,
  asset detail, originals, thumbnails; because the index decides and the
- **Album-scoped timeline + user directory**; the two calls the app's
  directory without verifiers; password hashes never leave the store,
- **Comprehensive E2E (release binary, curl as the client)**; the whole
  <root> <dir>`); point the vault at a photo directory and the dedup
  3, the server sees them; re-pointing at the same folder is a no-op,
- **Map markers + asset descriptions**; `GET /api/map/markers` serves the
  city/state/country are honest nulls; reversing a coordinate into a
  core now exists. `PUT /api/assets/{id}` gained `description`; the
- **Download archive**; `POST /api/download/archive` returns the selected
  a 2 GiB budget; a bigger selection is refused with "download in
- **Server surface completeness**; `GET /api/server/media-types` states
  `GET /api/config`; Immich's deep nested user-config object; is
- **Session revocation + brute-force lockout**; the security arc. Every
  enforced by the registry query; a user can only ever revoke their own.
  fifteen minutes, answering with the same error a wrong password gets -
## Research loop 2026-10-15/16: 10 iterations, 2 gaps closed
The Clawdius pattern applied to this repo: for every part; CAS dedup,
search, thumbnails, near-dup detection, transport; the loop pulled the
1. **Blob GC** (YP-CAS-GC-002); trash-empty removed rows while CAS
2. **SHA-1 ambiguity hardening** (YP-CRYPTO-SHA1-003); a SHA-1
   matches, and ambiguity answers **accept**; proven by
image-rs purity), PASETO (tokenkit already pins the algorithm; the
smart-search ML (pHash specified instead; YP-PIPELINE-PHASH-009, gated
  `aperture-pipeline::phash`; grayscale → 32×32 box-downsample →
  estate's own `dsp-spectral::dct2`; extracted from the MFCC path into
  threshold flips on float noise; Hamming 30 for two renders of the
  the phash column; caught by the memories/search/trash tests), and
   secret (a PAT with estate read access) exists; the workflow documents
2. **Auth is session+share only**; no OAuth, no email flows. The JWT
5. *(closed)* thumbhash shipped; placeholders ride the columnar
   transcodes** are the honest future case for worker-kit; a queue earns