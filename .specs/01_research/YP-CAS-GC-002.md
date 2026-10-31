# Yellow Paper: Garbage Collection: the Blob Leak the Sweep Never Covered
| git's reachability GC (gc.mark-and-sweep with grace periods) | Two-phase GC with an expiry grace period is simple, safe, and runs incrementally; the proven shape for content-addressed stores. |
| `cargo vet` / crate ecosystems aside; the CAS's own horizon sweeper (M0) | Sweeps *staging files* by age horizon. It never touches committed objects. Correct for its job; and never a GC. |
`trash_empty` removes index rows, memberships, and derived mappings -
the bytes" after the delete flow; the index-centric view had hidden it.
## YP-3: How the field solves it: and what we adopt
| Grace period before sweep | git (default 2 weeks) | Adopted in spirit: `gc` is explicit (`aperture-server gc`), never automatic; an operator runs it; no race with a concurrent ingest that has not yet committed its index row. |
## YP-4: Spec consequence: the invariant
`Pipeline::gc(store, index) -> GcReport`; mark-sweep over the objects