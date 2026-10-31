---
document_id: YP-PIPELINE-THUMBS-008
version: 1.0.0
status: APPROVED (cutting edge confirmed)
domain: Media Processing / Thumbnail Generation and Placeholders
subdomains: [ThumbHash vs BlurHash, libvips vs image-rs, Streaming Decode]
created: 2026-10-15
author: Loop
proof_files: []
confidence_level: 0.95
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: Thumbnails: Already on the Cutting Edge, Verified Against the Field

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| *ThumbHash: a more compact placeholder* (Evan Wallace, 2022, evanw.github.io) | ThumbHash stores per-channel low-frequency terms with aspect-ratio awareness: ~20–25 bytes, reproduces aspect ratio, beats BlurHash on detail and color fidelity per byte in the author's and community comparisons. |
| *BlurHash* (Kaarott/Bluespec 2018 spec) | The incumbent: DCT on 35-bit quantized components; aspect ratio must be stored separately and small thumbnails lose color accuracy. |
| libvips / sharp (Immich's own pipeline) | Streaming, demand-driven decode: thumbnails without materializing the full image; multithreaded; C; the industry standard for media servers. |
| `image` crate (Rust) | Pure Rust, zero unsafe, full decode into memory; single-threaded per image. ~2–4× slower and several× more memory than libvips on large JPEGs, but no FFI and no unsafe. |

## YP-2: The comparison verdict

| Decision | Field's answer | Vault's answer | Verdict |
|---|---|---|---|
| Placeholder format | BlurHash (older systems) / ThumbHash (newer) | **ThumbHash**, byte-compatible with the reference implementation | Already cutting edge; adopted in the ThumbHash arc with the byte-format argument |
| Decode/resize engine | libvips/sharp (C, streaming) | `image` crate (pure Rust, whole-image) | **Refused to switch.** `deny(unsafe_code)` and the pure-Rust estate rule outweigh a 2–4× thumbnail speedup on a single-user server where a thumbnail costs tens of milliseconds once per asset, cached forever after |
| Derivation caching | Immich: job queue + storage template | CAS blob per (source, spec), on-demand | The vault's design is *simpler* and cache-permanent; the M4 arc's decision stands (queue refused; video transcodes are the future worker-kit case) |

## YP-3: Are we solving a problem the field could not?

Not this one; and the paper says so plainly. ThumbHash *is* the
cutting edge (2022, successors of BlurHash), and the CAS-cached
on-demand derivation is a cleaner cache story than Immich's job queue
for stills. The field's libvips speed advantage is real and refused on
estate-purity grounds, with the refusal documented as the cost of
`deny(unsafe_code)`.

## YP-4: Spec consequence

No change. The paper exists so the next agent does not "modernize" the
placeholder to blurhash, or import a C imaging stack, without first
reading why the current position was chosen; and what measurement
(libvips > 2× sustained) would justify revisiting the engine refusal.
