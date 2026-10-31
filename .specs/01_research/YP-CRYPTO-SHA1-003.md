# Yellow Paper: SHA-1 Is a Hint: BLAKE3 Is the Identity
| *The First Collision for Full SHA-1* (Stevens, Karpman, Peyrin; "SHAttered", CRYPTO 2017) | Chosen-prefix-adjacent identical-prefix collision for full SHA-1, ~2⁶³ work: two *different PDFs*, same SHA-1, demonstrated in practice. |
| Immich's own use | Immich computes SHA-1 as the canonical per-asset checksum for client-side dedup; the protocol this vault speaks. Unchangeable from our side. |
   **duplicate**; "you already have B."
Severity analysis: the vault never corrupts a third party; bytes are
is impossible at check time; no bytes exist to compare. Cannot: drop
**ambiguity never skips.** When the lookup is not exactly-one; zero
matches, or several assets sharing one checksum; the answer is
`Cas.lean::different_bytes_different_addresses`; under the
- now enforced by construction, not by promise.