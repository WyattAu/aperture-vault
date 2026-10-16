---
document_id: YP-PIPELINE-PHASH-009
version: 1.0.0
status: APPROVED (implemented)
domain: Media Processing / Near-Duplicate Detection
subdomains: [Perceptual Hashing, DCT pHash, ML Embeddings]
created: 2026-10-15
author: Loop
proof_files: []
confidence_level: 0.90
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: Near-Duplicate Detection — pHash Is the ML-Free Cutting Edge

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| *Image Hashing for Copy Detection: a survey* (H. Farid, 2021) | The DCT-based pHash family remains the strong classical baseline for copy/near-duplicate detection; robust to resize, mild compression, small crops; Hamming distance thresholds separate near-duplicates. |
| Immich's duplicate detection | ML embeddings (CLIP-class transformer), cosine distance, a "duplicate detection" feature flag — flagship ML feature, GPU/model lifecycle included. |
| dsp-spectral / dsp-core (estate crates) | The estate already ships spectral primitives — the DCT machinery pHash needs. Dogfooding candidate: the algorithm is DSP the estate owns primitives for. |
| ThumbHash (YP-PIPELINE-THUMBS-008) | Already a low-frequency image descriptor — but tuned for *reconstruction*, not *matching*; not a substitute for pHash, though the machinery overlaps. |

## YP-2: The design, specified

`pHash(image)`: decode → grayscale 32×32 → DCT-II → take the 8×8
low-frequency block (skip the DC term) → median threshold → 64-bit
hash. Store alongside the thumbhash column. Duplicate candidates:
Hamming distance ≤ 10 (the community-standard threshold; ~10% bit
difference), surfaced as `GET /api/search/duplicates` — groups, not a
flat list.

The distance computation is a 64-bit popcount over a few thousand
rows: brute-force is correct at this scale; no ANN index (this is
where Immich needs pgvector and the vault does not).

## YP-3: Are we solving a problem the field could not?

The inverse, honestly stated: the field (Immich) solves semantic
duplicates ("different photos of the same dog") with ML. pHash solves
*perceptual* duplicates — the same image re-compressed, resized,
re-encoded, watermarked differently. That is the duplicate class a
photo vault actually needs (burst shots, exports, re-saves), and it
arrives without a model download, a GPU, or an inference lifecycle —
the exact trade Immich's ML stack pays and this vault refused.

## YP-4: Spec consequence — implemented

`aperture-pipeline::phash`: grayscale → 32×32 box-downsample →
separable 2-D DCT-II via the estate's `dsp-spectral::dct2` (the dogfood
the YP predicted — the transform was extracted from the MFCC path into
`dsp-spectral/src/dct.rs` for exactly this) → low-frequency 8×8 block
minus DC → median threshold → 64-bit hash, hex-stored in the `phash`
column (additive migration), computed on the same decode that yields
the thumbnail (M4's one-decode rule).

The fixture lesson is recorded because the first test image was a
pure linear gradient: a degenerate DCT where two coefficients carry all
the energy and the median threshold flips on float noise — Hamming 30
for two renders of the SAME image. The corrected fixture is a 1/f
spectral scene (amplitude ~ 1/(kx+ky+1), deterministic phases): with a
photo-like spectrum, resize and brightness perturbations sit within the
≤10 threshold and a checkerboard sits far outside. Synthetic-image
hash tests must use photo-like spectra or they test the fixture, not
the algorithm.

`GET /api/search/duplicates` and the burst-corpus threshold tuning
remain the next implementation slice; `near_duplicate` (Hamming ≤ 10)
is the exported rule.
