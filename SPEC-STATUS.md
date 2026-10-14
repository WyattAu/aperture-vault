# Status — after the ThumbHash milestone (2026-10-14)

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

- **M4** Thumbnails in `aperture-pipeline::thumb` + the Immich route
  `GET /api/assets/{id}/thumbnail?size=thumbnail|preview`. Generation is
  **synchronous on-demand with a deterministic cache** — a deliberate
  refusal of a job queue: a 250 px JPEG resize costs tens of milliseconds,
  and a queue would add a worker, a table, and retry semantics to defer
  work that is already instant. The derived blob is a CAS object in its
  own right (dedup, integrity, GC all follow); the index's new `derived`
  table maps `(source, spec)` to the derived address, so the cost is paid
  once per asset per size, forever. Longest edge bounded (250/1440 px),
  aspect preserved, small originals never upscaled, non-images answered
  with the 404 that is literally true. Blurhash is an honest not-yet.

- **M5 — planned as uring-kit under the CAS, refuted by measurement, pivoted
  to the Immich backup flow.** The measurement (kept as
  `crates/pipeline/examples/ingest_profile.rs`): a 100 MiB ingest spends
  ~100 ms reading, ~50 ms hashing (BLAKE3), ~110 ms writing. io_uring
  removes syscall *entry* cost — ~2 µs per op, ~6 ops per ingest — so its
  ceiling on the whole path is ~12 µs of 266 ms, 0.0045%. The cost is the
  page-cache copies, and the CAS must copy: BLAKE3 needs the bytes in
  userspace to hash them. uring-kit stays what it is — a socket substrate
  for shallow-poll edge serving, not a file-I/O accelerator.
- **Immich backup flow, grounded in the real OpenAPI spec** (193 paths,
  fetched from immich-app/immich): `GET /api/server/ping|features|config|
  version` — the connect-time handshake, every feature flag stating only
  what is true (password login yes, ML/OAuth/trash no), and the vault's
  own version rather than a pretended Immich one. `POST /api/assets/
  bulk-upload-check` — the call that makes backup resumable: the phone
  sends SHA-1s, the vault answers duplicate-with-id or accept. Ingest now
  computes SHA-1 in the same streaming pass as BLAKE3 (a hashing tee), and
  the index gained the `sha1` column (with an in-place migration tested
  against a real M2-era database) plus `asset_by_sha1`. SHA-1 is a compat
  alias, never identity: the CAS stays BLAKE3-only. `POST /api/assets` now
  returns Immich's `{id, status: created|duplicate}` — `duplicate` is
  confirmation, not an error. `GET /api/timeline/buckets` and
  `GET /api/timeline/bucket` serve the gallery in Immich's columnar shape
  (position-aligned arrays), same capture-day rule as the native timeline,
  with honest empty columns (thumbhash, city, country) where features
  do not exist yet.

- **Albums** — the app's album screens, grounded in the same OpenAPI spec:
  `GET/POST /api/albums`, `GET/PATCH/DELETE /api/albums/{id}`,
  `PUT/DELETE /api/albums/{id}/assets` with Immich's per-id
  `BulkIdResponseDto` outcomes. The index owns the truth: an `albums`
  table and an `album_assets` join table, ids from SQLite's own
  `randomblob`, covers from the first member added. The invariants the
  tests pin: an album is a *view* over assets — it never gains phantom
  members, deleting it never touches bytes, and re-adding a member is not
  double membership. Sharing fields (`albumUsers`, `shared`) ship at their
  neutral values: shape-complete, feature-honest.

- **Dogfooded against the real binary** (release build, curl as the
  client, ffmpeg-generated JPEGs as the library): `add-user` CLI (which
  dogfooding found missing — the config endpoint promised it), connect
  handshake, login, checksum dedup (accept before upload, reject/
  duplicate with the asset id after), upload returning
  `created`/`duplicate`, byte-identical CAS dedup across two "different"
  files, sentinel-day bucket for EXIF-less images, preview thumbnails
  served as real JPEGs and cache-identical on the second fetch, album
  create-seeded-and-listed over the wire. The whole flow a phone runs,
  working against the shipped binary on Immich's own default port.

- **ThumbHash placeholders** — the last visual gap. One decode of the
  original now yields both derived artifacts: the sized JPEG thumbnail and
  the placeholder. The reference downsamples to 100 px before encoding;
  the pipeline does the same, so hashes are byte-compatible with what
  clients generate. `thumbhash = "0.1"` is evanw's own algorithm — pure
  Rust, zero unsafe, and the wire format is the spec, so a hand-rolled
  DSP variant would break client compat, not express it. The hash rides
  the columnar timeline (`thumbhash` column, honestly empty until the
  first derivation fills it) and is stored via an additive migration.

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
5. *(closed)* thumbhash shipped — placeholders ride the columnar
   timeline; dogfooded against the release binary (28-char base64, 19
   bytes of binary, column-aligned).
3. **Thumbnails** (M4) ship for stills; **video poster frames and
   transcodes** are the honest future case for worker-kit — a queue earns
   its weight when generation is slow, and video is. Derived stills are
   already addressable CAS blobs, so the storage needs no migration.
4. **uring-kit** substrate: refuted for the CAS's file I/O by measurement
   (see M5 above); its honest future use is the socket path, if the vault
   ever outgrows axum/tokio for concurrent client serving.
