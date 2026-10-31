# Yellow Paper: Near-Duplicate Detection: pHash Is the ML-Free Cutting Edge
| Immich's duplicate detection | ML embeddings (CLIP-class transformer), cosine distance, a "duplicate detection" feature flag; flagship ML feature, GPU/model lifecycle included. |
| dsp-spectral / dsp-core (estate crates) | The estate already ships spectral primitives; the DCT machinery pHash needs. Dogfooding candidate: the algorithm is DSP the estate owns primitives for. |
| ThumbHash (YP-PIPELINE-THUMBS-008) | Already a low-frequency image descriptor; but tuned for *reconstruction*, not *matching*; not a substitute for pHash, though the machinery overlaps. |
difference), surfaced as `GET /api/search/duplicates`; groups, not a
*perceptual* duplicates; the same image re-compressed, resized,
arrives without a model download, a GPU, or an inference lifecycle -
## YP-4: Spec consequence: implemented
the YP predicted; the transform was extracted from the MFCC path into
the energy and the median threshold flips on float noise; Hamming 30