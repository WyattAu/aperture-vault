/-
  Lean4 spec model: sessions, revocation, brute-force lockout
  Component: aperture-auth
  Yellow Papers: YP-AUTH-REVOCATION-005, YP-AUTH-BRUTEFORCE-006

  Mirrors crates/auth/src/{service,sessions,users}.rs. Sync invariant:
  `validate` = (signature check) ∧ (registry liveness); `logout` removes
  the row; `lockout` answers the same error a wrong password gets.
-/

namespace Aperture.Auth

/-- Outcome of a login attempt. The spec only needs the error to be
*one value*: the vault's whole anti-enumeration posture is that there is
exactly one failure. -/
inductive Outcome where
  | ok : Outcome
  | fail : Outcome

 deriving DecidableEq

/-- A session id (`jti`). -/
abbrev Jti := String

/-- The registry: issued-and-not-yet-revoked session ids. -/
abbrev Registry := Jti → Bool

/-- The cryptographic half: is the signature valid? Modelled opaquely —
the spec does not re-derive HS256. -/
axiom signatureOk : String → Bool

/-- Validate a session token: signature **and** registry liveness.
Mirrors `Auth::validate_session`. -/
noncomputable def validate (reg : Registry) (token : String) (jtiOf : String → Jti) : Outcome :=
  if signatureOk token && reg (jtiOf token) then Outcome.ok else Outcome.fail

/-- Logout: remove the row. Returns the registry after removal — removal
of an absent id changes nothing (the honest no-op). Mirrors
`Auth::logout` + `SessionStore::revoke`. -/
def logout (reg : Registry) (token : String) (jtiOf : String → Jti) : Registry :=
  fun j => !(j == jtiOf token) && reg j

/-- **Theorem (revocation soundness).** After logging a token out, that
token no longer validates — for *any* jti mapping, because the removed
row was the only thing making it live. This is the machine-checked form
of "logout actually logs out". -/
theorem logout_revokes (reg : Registry) (token : String) (jtiOf : String → Jti)
    (_live : validate reg token jtiOf = Outcome.ok) :
    validate (logout reg token jtiOf) token jtiOf = Outcome.fail := by
  unfold validate logout
  rcases Classical.em (signatureOk token = true) with hsig | hsig
  · simp [hsig]
  · simp [hsig]

/-- A second logout changes nothing further: the row is already gone.
Mirrors the endpoint answering `successful: false`, not an error. -/
theorem logout_twice_idempotent (reg : Registry) (token : String) (jtiOf : String → Jti) :
    logout (logout reg token jtiOf) token jtiOf = logout reg token jtiOf := by
  unfold logout
  funext j
  simp [Bool.and_assoc, Bool.and_self]

/-- Lockout model: five consecutive failures arm a lockout, and while it
is armed the login outcome is `fail` **regardless of the password**. The
anti-enumeration property, machine-checked: a locked account answers the
right password exactly as it answers a wrong one. -/
structure LockState where
  failures : Nat
  locked : Bool

/-- The step function: failure increments (and locks at five); success
forgives, but only when not locked. Mirrors `UserStore::note_failure` +
the gate in `verify_login`. -/
def step (state : LockState) (success : Bool) : LockState :=
  if success && !state.locked then ⟨0, false⟩
  else if state.locked then state
  else
    let failures := state.failures + 1
    if failures ≥ 5 then ⟨0, true⟩ else ⟨failures, false⟩

/-- The outcome seen by the caller while locked. -/
noncomputable def lockedOutcome (state : LockState) (passwordOk : Bool) : Outcome :=
  if state.locked then Outcome.fail
  else if passwordOk && !state.locked then Outcome.ok else Outcome.fail

/-- **Theorem (no lockout oracle).** While locked, the right password and
a wrong password produce the *same* outcome. A lockout that treated them
differently would be an account-existence oracle. -/
theorem locked_has_no_oracle (state : LockState) (passwordOk : Bool)
    (h : state.locked = true) :
    lockedOutcome state passwordOk = Outcome.fail := by
  unfold lockedOutcome
  simp [h]

/-- **Theorem (the right password forgives the typos).** When not locked,
a correct login resets the failure ledger — the counter never accumulates
across a success. -/
theorem success_forgives (state : LockState) (h : state.locked = false) :
    step state true = ⟨0, false⟩ := by
  unfold step
  simp [h]

end Aperture.Auth
