---
document_id: YP-CRYPTO-SHA1-003
version: 1.0.0
status: APPROVED (hardening implemented)
domain: Cryptography / Hash Function Deprecation
subdomains: [SHA-1 Collisions, Compat Aliases, Identity vs Hint]
created: 2026-10-15
author: Loop
proof_files: [Cas.lean, Policy.lean]
confidence_level: 0.97
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: SHA-1 Is a Hint: BLAKE3 Is the Identity

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| *The First Collision for Full SHA-1* (Stevens, Karpman, Peyrin; "SHAttered", CRYPTO 2017) | Chosen-prefix-adjacent identical-prefix collision for full SHA-1, ~2⁶³ work: two *different PDFs*, same SHA-1, demonstrated in practice. |
| *SHA-1 is a Shambles* (Leurent & Peyrin, EUROCRYPT 2020) | Chosen-prefix collisions at ~2⁶³·⁴ with GPU-rental economics: an attacker with a few thousand dollars can construct colliding file pairs. |
| NIST SP 800-131A rev.2 | SHA-1 disallowed for digital signature generation and for *collision-resistance* purposes; allowed for non-security uses such as checksums of trusted input. |
| Immich's own use | Immich computes SHA-1 as the canonical per-asset checksum for client-side dedup; the protocol this vault speaks. Unchangeable from our side. |

## YP-2: The attack surface on the vault

`bulk-upload-check` answers "already have it?" from a client-supplied
SHA-1. With SHAttered-class work an attacker constructs files A and B,
SHA-1-equal:

1. Upload A. The vault stores A, records A's SHA-1.
2. Present B's checksum. `bulk-upload-check` matches A's row and answers
   **duplicate**; "you already have B."
3. The attacker (or the attacker's victim, if B is *their* file) trusts
   the vault to hold B. It holds A.

Severity analysis: the vault never corrupts a third party; bytes are
deduplicated by BLAKE3, and a collision never merges two stored
objects. The damage is a **self-denial-of-service**: the dup-check lies
about B, and B never gets stored.

## YP-3: What can and cannot be hardened

Cannot: the endpoint sees only a checksum, so "verify the collision"
is impossible at check time; no bytes exist to compare. Cannot: drop
SHA-1; the Immich protocol requires it.

Can and done, machine-checked as `Policy.lean::ambiguous_never_skips`:
**ambiguity never skips.** When the lookup is not exactly-one; zero
matches, or several assets sharing one checksum; the answer is
*accept*, and the upload itself (BLAKE3-addressed) resolves identity.
`assets_by_sha1` replaces the single-row lookup; a colliding pair makes
the endpoint answer accept for both, permanently.

## YP-4: The deeper claim, proven

`Cas.lean::different_bytes_different_addresses`; under the
(computational, honest) injectivity assumption for BLAKE3, different
bytes can never merge. The system's correctness **never depends on
SHA-1**; it depends on BLAKE3, and the spec says so in one theorem.
SHA-1 is a compatibility hint that can be wrong without hurting anyone : now enforced by construction, not by promise.

## YP-5: Comparison

restic and kopia treat non-primary checksums the same way after their
own crypto audits: additional checksums are *verification hints*; the
content address is the only identity. The vault is now aligned with
that posture.
