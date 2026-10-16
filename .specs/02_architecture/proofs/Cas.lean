/-
  Lean4 spec model: the content-addressed store
  Component: aperture-cas
  Yellow Papers: YP-CAS-CHUNKING-001, YP-CAS-GC-002

  The store is modelled as the characteristic predicate of the addresses
  it holds — the spec-level truth the implementation approximates with a
  sharded directory of BLAKE3-named files. The rules proven here
  (idempotence of ingest, address separation, GC safety) are the rules
  the Rust implementation must satisfy; the concrete file-walk is covered
  by its own tests.
-/

namespace Aperture.Cas

/-- Bytes, modelled as lists of byte values. -/
abbrev Bytes := List Nat

/-- A content address: BLAKE3 of the bytes, modelled opaquely. -/
axiom address : Bytes → Nat

/-- Determinism axiom (BLAKE3 is a pure function of the bytes — no salt,
no clock, no key): the same bytes always produce the same address. The
axiom the whole dedup design stands on. -/
axiom address_deterministic (b : Bytes) : address b = address b

/-- Injectivity **assumption**, not a theorem: computational, never
provable. Claims that need it cite this axiom honestly. -/
axiom address_injective : ∀ a b : Bytes, address a = address b → a = b

/-- The store: the characteristic predicate of held addresses. -/
abbrev Store := Nat → Bool

/-- Whether the store holds an address. -/
def holds (store : Store) (addr : Nat) : Bool :=
  store addr

/-- Ingest: the store after the write — everything it held, plus the
new bytes' address. `INSERT OR IGNORE` semantics are visible in the
idempotence theorem below. -/
noncomputable def ingest (store : Store) (b : Bytes) : Store :=
  fun a => store a || a == address b

/-- **Theorem (dedup by construction).** Ingesting the same bytes twice
leaves the store unchanged after the first. Machine-checked form of the
CAS headline claim: `newly_written == false` on the second ingest. -/
theorem ingest_is_idempotent (store : Store) (b : Bytes) :
    ingest (ingest store b) b = ingest store b := by
  unfold ingest
  funext a
  simp only [Bool.or_assoc, Bool.or_self]

/-- **Theorem (different bytes, different addresses).** Under the
injectivity assumption, different bytes claim different addresses.
Honest reading: modulo the computational assumption; a SHA-1-style break
would void it — which is why YP-CRYPTO-SHA1-003 keeps BLAKE3 as the only
identity and demotes SHA-1 to a hint that can be wrong without hurting
anyone. -/
theorem different_bytes_different_addresses (a b : Bytes) (hne : a ≠ b) :
    address a ≠ address b := fun h => hne (address_injective a b h)

/-- The garbage collector: the sweep keeps exactly the *marked* addresses
(the index's reachability set). Mirrors `Pipeline::gc` (YP-CAS-GC-002);
the concrete directory walk is the implementation's job. -/
def gcSweep (marked : Nat → Bool) : Store := marked

/-- **Theorem (GC safety).** Sweeping keeps every marked object. The rule
the implementation must satisfy: after `Pipeline::gc`, every address the
index holds resolves to bytes on disk. Written against the obnam v1
failure, where a GC lost reachable data. -/
theorem gc_safety (marked : Nat → Bool) :
    ∀ addr, marked addr = true → holds (gcSweep marked) addr = true := by
  intro addr hm
  simp [gcSweep, holds, hm]

end Aperture.Cas
