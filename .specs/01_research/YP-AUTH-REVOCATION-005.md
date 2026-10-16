---
document_id: YP-AUTH-REVOCATION-005
version: 1.0.0
status: APPROVED (implemented + proven)
domain: Authentication / Session Management
subdomains: [JWT, Stateful Revocation, Argon2id, RFC 8725]
created: 2026-10-15
author: Loop
proof_files: [Auth.lean]
confidence_level: 0.96
tqa_level: 3
research_iterations: 3
---

# Yellow Paper: Session Revocation — the Registry Between the Signature and the Answer

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| RFC 9106 (Argon2, 2021) | Argon2id with m ≥ 19 MiB, t ≥ 2, p ≥ 1 is the recommended memory-hard KDF. The estate `salting` crate's parameters were checked against this. |
| RFC 8725 *Best Current Practices for JWT* | The JWT failure class is algorithm confusion ("alg: none", RS/HS downgrade) and unvalidated claims. BCP: pin the algorithm, validate issuer/audience/expiry, never trust the header. |
| PASETO v4 (Oasis design docs) | PASETO removes algorithm choice entirely — the anti-JWT. The honest comparison: tokenkit already pins HS256 (no header trust), which is the BCP-compliant subset of JWT. |
| OWASP Session Management Cheat Sheet | Session tokens must be revocable server-side; logout must invalidate; idle+absolute expiry both recommended. |
| OAuth 2.0 token revocation (RFC 7009) | Revocation = a registry lookup, not token surgery. Exactly the SessionStore shape. |

## YP-2: What iteration found

The M2 design minted stateless JWTs: signature-valid forever, so
"logout" could only lie, and a stolen token was unrecoverable without
regenerating the signing secret (nuking every session). Iteration 3
implemented the fix, and the model in `proofs/Auth.lean` proves the
property the implementation must carry:

- `validate = signatureOk ∧ registry-live` — the JWT proves
  authenticity; the registry decides liveness.
- `logout_revokes`: after logout, the token fails validation *for any*
  jti mapping — revocation is not a UI nicety, it is the registry row
  disappearing.
- `logout_twice_idempotent`: a second logout changes nothing —
  `successful: false`, not an error.

## YP-3: Are we solving a problem the field could not?

No — the field (RFC 7009) solved it in 2012. The contribution is that
this vault had the gap and closed it with the standard shape, proving
the closure in the proof assistant rather than asserting it in a test.
The tests additionally pin the wire behaviors (401 after logout,
`successful: false` on the no-op) so the model and the API cannot drift.

## YP-4: Spec consequence

1. `Auth` issues `jti`-stamped sessions and consults the `SessionStore`
   on every validation. Issuance records; revocation deletes; expiry
   purges (on open) and gates (on read).
2. `POST /api/auth/logout` is registry-backed. `GET /api/sessions` lists
   the operator's own rows; `DELETE /api/sessions/{id}` is
   ownership-enforced by the registry query.
3. Argon2id parameters stay per RFC 9106 via the estate `salting` crate
   (verified: memory-hard, salted, PHC-string output).
4. Tokenkit's pinned HS256 satisfies RFC 8725 by construction: the
   header's `alg` is never consulted. PASETO migration is refused —
   the failure class is already excluded.
