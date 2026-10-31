# Yellow Paper: Thumbnails: Already on the Cutting Edge, Verified Against the Field
| libvips / sharp (Immich's own pipeline) | Streaming, demand-driven decode: thumbnails without materializing the full image; multithreaded; C; the industry standard for media servers. |
| Placeholder format | BlurHash (older systems) / ThumbHash (newer) | **ThumbHash**, byte-compatible with the reference implementation | Already cutting edge; adopted in the ThumbHash arc with the byte-format argument |
Not this one; and the paper says so plainly. ThumbHash *is* the
reading why the current position was chosen; and what measurement