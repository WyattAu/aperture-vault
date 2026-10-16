---
document_id: YP-AUTH-BRUTEFORCE-006
version: 1.0.0
status: APPROVED (implemented + proven)
domain: Authentication / Online Guessing Defence
subdomains: [Lockout Policies, Anti-Enumeration, NIST 800-63B]
created: 2026-10-15
author: Loop
proof_files: [Auth.lean]
confidence_level: 0.95
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: Brute-Force Lockout — Silence Without an Oracle

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| NIST SP 800-63B §5.2.2 (2017) | Verifiers **SHALL** implement rate limiting / throttling against online guessing. Account lockout for *extended* periods is discouraged (denial-of-service vector); throttling with escalating delay is the recommended shape. |
| OWASP Authentication Cheat Sheet | 5–10 attempts → lockout or escalating delay; the error must not distinguish "unknown user" from "wrong password"; consider CPU-expensive hash on every path (already: the vault burns Argon2 on unknown users). |
| *The Quest to Replace Passwords* (Bonneau et al., 2012 survey) | Lockouts trade availability for security; the DoS-ability of a *named* account is the classic complaint — the countermeasure is uniform errors + bounded lockout windows. |
| RFC 8259-era PoW login schemes (hashcash-style login puzzles) | Proof-of-work per attempt defends without user-visible lockout — costs client complexity and excludes weak clients. Refused for a home server. |

## YP-2: The design tension, stated

A lockout that says "locked" leaks that the account exists. The vault's
answer, machine-checked in `Auth.lean::locked_has_no_oracle`: **while
locked, the right password and a wrong password produce the same
outcome.** The caller learns nothing except that login failed. The
residual is timing: a locked check returns without burning Argon2
(microseconds vs ~100 ms), which is the documented residual — closing
it (burn a dummy hash on the locked path) would cost nothing and is
worth doing if timing attacks on the wire are ever in the model.

## YP-3: The policy, and its proven siblings

- 5 consecutive failures → 15 minutes of lockout. Bounded window per
  SP 800-63B (not the permanent lockout the cheat sheet warns about).
- A success forgives the ledger (`Auth.lean::success_forgives`): one
  good password erases the typo count.
- The ledger is in-memory: an attacker who can restart the server has
  already won something worse than a password. Statefulness across
  restarts was refused because it adds a persistence surface for zero
  additional defence against the online attacker.
- Unknown users burn a hash on every attempt (M2 defence, retained):
  timing of a miss for an unknown name matches a wrong password.

## YP-4: Spec consequence

`UserStore::verify_login` gates on the lockout, records failures, and
clears on success. The outcome type has exactly one failure value — the
whole anti-enumeration posture is that there is exactly one failure —
and the theorem `locked_has_no_oracle` is the compiled statement of
that posture.
