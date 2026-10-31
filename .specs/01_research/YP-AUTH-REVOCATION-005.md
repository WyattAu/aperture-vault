# Yellow Paper: Session Revocation: the Registry Between the Signature and the Answer
| PASETO v4 (Oasis design docs) | PASETO removes algorithm choice entirely; the anti-JWT. The honest comparison: tokenkit already pins HS256 (no header trust), which is the BCP-compliant subset of JWT. |
- `validate = signatureOk ∧ registry-live`; the JWT proves
  jti mapping; revocation is not a UI nicety, it is the registry row
- `logout_twice_idempotent`: a second logout changes nothing -
No; the field (RFC 7009) solved it in 2012. The contribution is that
   header's `alg` is never consulted. PASETO migration is refused -