# Status: after the ThumbHash milestone (2026-10-14)

## Shipped

- **M0** `aperture-cas`; BLAKE3-addressed store: streaming ingest (bounded
  memory, hashed while written), atomic same-directory rename, dedup by
  construction, corruption detection, sharded layout, horizon sweeper.
- **M1** `aperture-probe`; JPEG/EXIF (both endiannesses, GPS DMS→decimal),
  PNG IHDR dimensions, MP4 duration/dimensions via estate mp4-core. Total
  parsing: corrupt bytes yield an empty record, never a failed listing.
- **M2** `aperture-index`; SQLite/WAL asset index keyed by content address;
  timeline newest-first by capture day, sentinel-day grouping for untagged
  assets, stable pagination. `aperture-auth`; salted Argon2 verifiers
  (estate salting), session JWTs, share-link capability tokens (estate
  tokenkit), account-enumeration defence, minimum-length password policy.
  `aperture-pipeline`; the ingest junction: hash-while-stream → dedup
  short-circuit → probe what survived → index what probed.
- **M3** Immich-compatible API in `aperture-server`; the protocol the
  existing Immich mobile apps speak, so clients attach unchanged:
  `POST /api/auth/login` (`{email, password}` → `{accessToken, userId}`,
  `username` accepted as an alias; one login endpoint serves both client
  generations), `POST /api/auth/validateToken`, `GET /api/users/me` (full
  profile via the new `UserStore::user_by_id`), multipart `assetData`
  upload, and `GET /api/assets/{id}/original`. One upload URL serves both
  encodings: a dispatcher branches on Content-Type, handing multipart
  bodies to the Immich path and raw bytes + `?filename=` to the native
  path, so each client keeps its exact contract at the same endpoint.
  Asset ids are content addresses, not UUIDs; clients treat ids as
  opaque strings, and an id that lies about identity would be a defect.

- **M4** Thumbnails in `aperture-pipeline::thumb` + the Immich route
  `GET /api/assets/{id}/thumbnail?size=thumbnail|preview`. Generation is
  **synchronous on-demand with a deterministic cache**; a deliberate
  refusal of a job queue: a 250 px JPEG resize costs tens of milliseconds,
  and a queue would add a worker, a table, and retry semantics to defer
  work that is already instant. The derived blob is a CAS object in its
  own right (dedup, integrity, GC all follow); the index's new `derived`
  table maps `(source, spec)` to the derived address, so the cost is paid
  once per asset per size, forever. Longest edge bounded (250/1440 px),
  aspect preserved, small originals never upscaled, non-images answered
  with the 404 that is literally true. Blurhash is an honest not-yet.

- **M5; planned as uring-kit under the CAS, refuted by measurement, pivoted
  to the Immich backup flow.** The measurement (kept as
  `crates/pipeline/examples/ingest_profile.rs`): a 100 MiB ingest spends
  ~100 ms reading, ~50 ms hashing (BLAKE3), ~110 ms writing. io_uring
  removes syscall *entry* cost; ~2 µs per op, ~6 ops per ingest; so its
  ceiling on the whole path is ~12 µs of 266 ms, 0.0045%. The cost is the
  page-cache copies, and the CAS must copy: BLAKE3 needs the bytes in
  userspace to hash them. uring-kit stays what it is; a socket substrate
  for shallow-poll edge serving, not a file-I/O accelerator.
- **Immich backup flow, grounded in the real OpenAPI spec** (193 paths,
  fetched from immich-app/immich): `GET /api/server/ping|features|config|
  version`; the connect-time handshake, every feature flag stating only
  what is true (password login yes, ML/OAuth/trash no), and the vault's
  own version rather than a pretended Immich one. `POST /api/assets/
  bulk-upload-check`; the call that makes backup resumable: the phone
  sends SHA-1s, the vault answers duplicate-with-id or accept. Ingest now
  computes SHA-1 in the same streaming pass as BLAKE3 (a hashing tee), and
  the index gained the `sha1` column (with an in-place migration tested
  against a real M2-era database) plus `asset_by_sha1`. SHA-1 is a compat
  alias, never identity: the CAS stays BLAKE3-only. `POST /api/assets` now
  returns Immich's `{id, status: created|duplicate}`; `duplicate` is
  confirmation, not an error. `GET /api/timeline/buckets` and
  `GET /api/timeline/bucket` serve the gallery in Immich's columnar shape
  (position-aligned arrays), same capture-day rule as the native timeline,
  with honest empty columns (thumbhash, city, country) where features
  do not exist yet.

- **Albums**; the app's album screens, grounded in the same OpenAPI spec:
  `GET/POST /api/albums`, `GET/PATCH/DELETE /api/albums/{id}`,
  `PUT/DELETE /api/albums/{id}/assets` with Immich's per-id
  `BulkIdResponseDto` outcomes. The index owns the truth: an `albums`
  table and an `album_assets` join table, ids from SQLite's own
  `randomblob`, covers from the first member added. The invariants the
  tests pin: an album is a *view* over assets; it never gains phantom
  members, deleting it never touches bytes, and re-adding a member is not
  double membership. Sharing fields (`albumUsers`, `shared`) ship at their
  neutral values: shape-complete, feature-honest.

- **Dogfooded against the real binary** (release build, curl as the
  client, ffmpeg-generated JPEGs as the library): `add-user` CLI (which
  dogfooding found missing; the config endpoint promised it), connect
  handshake, login, checksum dedup (accept before upload, reject/
  duplicate with the asset id after), upload returning
  `created`/`duplicate`, byte-identical CAS dedup across two "different"
  files, sentinel-day bucket for EXIF-less images, preview thumbnails
  served as real JPEGs and cache-identical on the second fetch, album
  create-seeded-and-listed over the wire. The whole flow a phone runs,
  working against the shipped binary on Immich's own default port.

- **ThumbHash placeholders**; the last visual gap. One decode of the
  original now yields both derived artifacts: the sized JPEG thumbnail and
  the placeholder. The reference downsamples to 100 px before encoding;
  the pipeline does the same, so hashes are byte-compatible with what
  clients generate. `thumbhash = "0.1"` is evanw's own algorithm; pure
  Rust, zero unsafe, and the wire format is the spec, so a hand-rolled
  DSP variant would break client compat, not express it. The hash rides
  the columnar timeline (`thumbhash` column, honestly empty until the
  first derivation fills it) and is stored via an additive migration.

- **Share links**; the auth layer's tokenkit capability was minted and
  validated but served by nothing; now it is. `POST/GET /api/shared-links`
  and `DELETE /api/shared-links/{id}` for the owner; `GET
  /api/shared-links/me?key=` for the visitor; no session, album plus
  member ids. The vault's share model is exactly the substrate's:
  album-scoped, read-only, expiring. Revocation is real, not cosmetic:
  the JWT validates cryptographically forever, so the row is the source
  of truth; a key opens assets only while its row exists, and asset
  endpoints accept session *or* share-key, the latter reaching exactly
  the shared album's members. Roles are distinct by construction: a
  session token is not a share key and never authenticates as one.

- **Asset detail**; `GET /api/assets/{id}`, the record the app opens per
  photo: full `AssetResponseDto` shape (dimensions, EXIF block, placeholder,
  neutral feature fields), mime sniffed from the stored magic through a
  bounded 16-byte read; never the whole blob for a content type. The
  redundant native byte route (`GET /api/assets/{address}`) was folded
  into Immich's `GET /api/assets/{id}/original`: one bytes path, one
  detail path, no overlapping captures for axum to trip over.

- **Memories ("on this day")**; the feature that looks like ML and is
  actually a calendar query. `GET /api/memories` groups assets captured on
  today's month/day in previous years, one memory per year, newest first,
  Immich's `on_this_day` shape with `yearAgo`. The rule the tests pin:
  assets without capture dates are never memories; a guess would be a
  lie with a timestamp. Pure index arithmetic; the EXIF had it all along.

- **Favorite + statistics**; `PUT /api/assets/{id}` with the honest slice
  of `UpdateAssetDto` (the favorite toggle; description/rating/visibility
  refused by omission; accepted-and-ignored would lie), and `GET
  /api/assets/statistics` (`{images, videos, total}`), the counts the
  backup screen displays. Dogfooded on the release binary: star, detail
  agreement, statistics.

- **Search**; `POST /api/search/metadata` over the fields the vault
  truly indexes: file-name substring (case-insensitive, matching assets
  and album names), `IMAGE`/`VIDEO`, favorite. Immich's query DTO carries
  thirty-plus filters; the ones with no backing column are ignored, and
  the response shows it by not matching on them. The WHERE clause is
  assembled from numbered placeholders only; user input is always a
  bound value, never SQL text.

- **Trash**; `DELETE /api/assets` (trash; `force` skips to permanent),
  `GET /api/trash`, `POST /api/trash/restore/assets`, `POST
  /api/trash/restore`, `POST /api/trash/empty`. A trashed asset leaves
  every read; timeline, buckets, search, memories, statistics, albums,
  asset detail, originals, thumbnails; because the index decides and the
  CAS only stores. Restores put album memberships back untouched (they
  were never removed). Emptying is the one place the cache intentionally
  forgets: rows, memberships, and derived mappings go; the CAS blobs
  become unreferenced garbage for the horizon sweeper.

- **Album-scoped timeline + user directory**; the two calls the app's
  album screen and share dialog make: `GET /api/timeline/buckets?albumId=`
  and `.../bucket?albumId=` now narrow to album members (unknown album is
  a 404; an empty day is an empty bucket), and `GET /api/users` serves the
  directory without verifiers; password hashes never leave the store,
  asserted in the test by searching the wire format for the PHC prefix.

- **Comprehensive E2E (release binary, curl as the client)**; the whole
  surface in one session: three distinct uploads (plus an accidental
  re-demonstration of dedup: three identical ffmpeg frames collapsed to
  one asset), favorite, album buckets + album-scoped day fetch, search by
  name and favorite, trash → statistics drop → restore → counts return,
  share public access and 401-after-revoke, users directory with no
  verifier material, memories honestly empty without EXIF. Every read and
  every state change exercised over real HTTP against the shipped binary.

- **Folder scan** (`aperture-pipeline::scan` + `aperture-server scan
  <root> <dir>`); point the vault at a photo directory and the dedup
  machinery decides what is new. The skip check is the SHA-1 column in a
  new role: clients asked "did you get this already?" and now the scanner
  asks itself before writing. Two streaming passes (hash, then ingest
  only if unknown) beat buffering a video; hidden files are ignored;
  subdirectories are walked; one bad file lands in the failure list
  instead of sinking the run. Dogfooded: scan 1 ingests 3, scan 2 skips
  3, the server sees them; re-pointing at the same folder is a no-op,
  which is the whole point of content addressing.

- **Map markers + asset descriptions**; `GET /api/map/markers` serves the
  pins from the GPS the probe has extracted since M1 (coordinates only;
  city/state/country are honest nulls; reversing a coordinate into a
  name needs a gazetteer this vault does not ship, and the pin renders
  fine without one). `features.map` flipped to true because the map's
  core now exists. `PUT /api/assets/{id}` gained `description`; the
  caption is authorship, and camera-default EXIF descriptions are not
  imported. Unsupported update fields are refused **by name** in the
  error text, never silently ignored: an update that pretends to succeed
  while dropping a field is a lie.

- **Download archive**; `POST /api/download/archive` returns the selected
  originals as one ZIP, verified in tests by a real unzipper (the `zip`
  crate reading the vault's own bytes back). The writer is STORE-method
  and ~150 lines: photos are already compressed, and deflate on JPEG
  bytes spends CPU to save nothing. The archive buffers in memory behind
  a 2 GiB budget; a bigger selection is refused with "download in
  parts", which is the honest response. Duplicate display names get
  `name (1).ext` suffixes instead of silently overwriting; share keys
  reach exactly their album's members, as everywhere.

- **Server surface completeness**; `GET /api/server/media-types` states
  exactly what the probe parses (jpg/jpeg/png, mp4; sidecar empty), and
  `GET /api/server/statistics` reports assets by kind plus total bytes
  (`usagePhotos`/`usageVideos` carry the same byte total split
  optimistically; per-kind byte tracking wants a column, not a guess).
  `GET /api/config`; Immich's deep nested user-config object; is
  deliberately absent: nothing reads it before login, and a config
  surface nobody consumes is one more thing to keep honest.

- **Session revocation + brute-force lockout**; the security arc. Every
  issued session now carries a `jti` recorded in a new `SessionStore`
  (same atomic-write JSON pattern as the user registry): the JWT proves
  authenticity, the registry decides liveness, so `POST /api/auth/logout`
  revokes for real and a logged-out token stops working immediately (a
  second logout is `successful: false`, the honest no-op). `GET
  /api/sessions` lists the operator's live sessions with the caller's own
  marked `current`; `DELETE /api/sessions/{id}` revokes one, ownership
  enforced by the registry query; a user can only ever revoke their own.
  Login failures lock the account after five consecutive misses for
  fifteen minutes, answering with the same error a wrong password gets -
  a lockout that named itself would be an account-existence oracle; the
  timing difference (no hash burned) is the documented residual.

## Research loop 2026-10-15/16: 10 iterations, 2 gaps closed

The Clawdius pattern applied to this repo: for every part; CAS dedup,
GC, checksums, at-rest crypto, session auth, brute-force defence,
search, thumbnails, near-dup detection, transport; the loop pulled the
literature, compared against restic/borg/kopia/Immich, decided in a
Yellow Paper, proved the core claims in Lean, and implemented what won.
See `.specs/01_research/` (10 YPs + registry) and
`.specs/02_architecture/proofs/` (Cas.lean, Auth.lean, Policy.lean).

**Proof status: all three files verify with zero errors and zero
sorries** (`lake env lean <file>`): dedup idempotence, address
separation under the (declared) injectivity assumption, GC safety,
revocation soundness, logout idempotence, no-lockout-oracle,
success-forgives, share scoping, trash exclusion, and
ambiguity-never-skips. The sync invariant stands: these models mirror
the Rust types, and a Rust change that breaks a mirrored shape must
update its proof file in the same commit.

Gaps the loop found and closed:

1. **Blob GC** (YP-CAS-GC-002); trash-empty removed rows while CAS
   blobs leaked forever; the M0 sweeper only collects staging files.
   `Pipeline::gc` + `aperture-server gc`: mark-sweep from the index's
   reachability set (assets ∪ derived), operator-run, proven by
   `gc_safety` in the proof suite. Dogfooded: orphan removed, marked
   originals still serve.
2. **SHA-1 ambiguity hardening** (YP-CRYPTO-SHA1-003); a SHA-1
   collision could make bulk-upload-check claim "already have it" about
   bytes the vault does not hold. Now: `assets_by_sha1` returns *all*
   matches, and ambiguity answers **accept**; proven by
   `ambiguous_never_skips` in Policy.lean.

Refusals with reasons (so the next agent reads, not re-litigates):
content-defined chunking (photos are immutable; CDC wins on deltas the
workload lacks), at-rest application-layer crypto (LUKS posture; age
becomes mandatory with any remote backend), libvips (unsafe/C vs
image-rs purity), PASETO (tokenkit already pins the algorithm; the
RFC 8725 subset), FTS5 (tripwire: ~10⁷ rows or double-digit-ms scans),
smart-search ML (pHash specified instead; YP-PIPELINE-PHASH-009, gated
on burst-corpus testing).

- **pHash shipped** (YP-PIPELINE-PHASH-009, implemented from the spec):
  `aperture-pipeline::phash`; grayscale → 32×32 box-downsample →
  separable 2-D DCT-II → low-frequency 8×8 block minus DC → median
  threshold → 64-bit hash, hex-stored (additive migration) and computed
  on the same decode that yields the thumbnail. The transform is the
  estate's own `dsp-spectral::dct2`; extracted from the MFCC path into
  a standalone primitive for exactly this, the dogfood the YP predicted.
  The fixture lesson is in the YP: a linear-gradient test image has a
  degenerate DCT (two coefficients carry everything; the median
  threshold flips on float noise; Hamming 30 for two renders of the
  SAME image). The corrected 1/f spectral fixture keeps photo-like
  coefficient spread: resize and brightness perturbations within the
  ≤10 threshold, a checkerboard far outside. `near_duplicate` (Hamming
  ≤ 10) is the exported rule; `GET /api/search/duplicates` is the next
  slice. The wide-table tax struck again (three SELECTs drifted without
  the phash column; caught by the memories/search/trash tests), and
  the fix is structural: one `ASSET_COLUMNS` constant + one
  `read_asset` row mapper, so the next column is two edits, not six
  hand-copied queries.

## Dogfood used

`mp4-core` (video probe; contributed `read_video_dimensions_in` upstream),
estate CI gate, `salting` + `tokenkit` from crates.io (the same pins
`estate-integration` verifies).

## Honest open items

1. **CI fails at the private-sibling checkout** until an `ESTATE_TOKEN`
   secret (a PAT with estate read access) exists; the workflow documents
   this at the failing step. mp4-core, id3-core, audiobook-core and
   audiobook-conformance are private; a public vault whose dependencies are
   private repos is one nobody else can build, which is a visibility
   decision above this repository.
2. **Auth is session+share only**; no OAuth, no email flows. The JWT
   substrate makes adding them mechanical.
5. *(closed)* thumbhash shipped; placeholders ride the columnar
   timeline; dogfooded against the release binary (28-char base64, 19
   bytes of binary, column-aligned).
3. **Thumbnails** (M4) ship for stills; **video poster frames and
   transcodes** are the honest future case for worker-kit; a queue earns
   its weight when generation is slow, and video is. Derived stills are
   already addressable CAS blobs, so the storage needs no migration.
4. **uring-kit** substrate: refuted for the CAS's file I/O by measurement
   (see M5 above); its honest future use is the socket path, if the vault
   ever outgrows axum/tokio for concurrent client serving.
