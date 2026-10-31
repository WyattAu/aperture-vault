# Yellow Paper: Brute-Force Lockout: Silence Without an Oracle
| *The Quest to Replace Passwords* (Bonneau et al., 2012 survey) | Lockouts trade availability for security; the DoS-ability of a *named* account is the classic complaint; the countermeasure is uniform errors + bounded lockout windows. |
| RFC 8259-era PoW login schemes (hashcash-style login puzzles) | Proof-of-work per attempt defends without user-visible lockout; costs client complexity and excludes weak clients. Refused for a home server. |
(microseconds vs ~100 ms), which is the documented residual; closing
clears on success. The outcome type has exactly one failure value; the
whole anti-enumeration posture is that there is exactly one failure -