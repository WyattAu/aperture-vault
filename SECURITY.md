# Security Policy: mp4-core
- **No network, filesystem, or process state**; the crate is pure
- `#![forbid(unsafe_code)]`; no unsafe blocks exist in this crate.