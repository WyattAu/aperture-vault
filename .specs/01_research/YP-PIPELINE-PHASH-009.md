---
document_id: YP-PIPELINE-PHASH-009
version: 1.0.0
status: APPROVED (spec'd, implementation deferred)
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

## YP-4: Spec consequence — deferred, deliberately

The YP is the spec; implementation is deferred behind two gates that
this loop refuses to skip:

1. The decode cost: pHash needs a full decode per image. It rides the
   thumbnail-derivation path (one decode, three artifacts — M4's rule)
   *after* the column migration, exactly like thumbhash did.
2. The threshold is workload-dependent: the 10-bit default ships behind
   a test on a burst-shot corpus before the endpoint exists.

Deferred, not refused: the YP is the anchor so the implementation arc
starts from the design, not from a search.
