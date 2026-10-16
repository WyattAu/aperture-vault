/-
  Lean4 spec model: the content-addressed store
  Component: aperture-cas
  Yellow Papers: YP-CAS-CHUNKING-001, YP-CAS-GC-002

  Mirrors crates/cas/src/lib.rs and Pipeline::gc (crates/pipeline).
  Sync invariant: `address`, `ingest`, and `gcSweep` must track the Rust
  implementation; gc_safety below is the rule YP-CAS-GC-002 imposes on
  the implementation (written against the obnam v1 failure, where a GC
  lost reachable data).
-/

namespace Aperture.Cas

/-- Bytes, modelled as lists of byte values. -/
abbrev Bytes := List Nat

/-- A content address: BLAKE3 of the bytes, modelled opaquely. The only
property the spec needs is determinism. -/
axiom address : Bytes → Nat

/-- Determinism axiom (BLAKE3 is a pure function of the bytes — no salt,
no clock, no key): the same bytes always produce the same address. This
is the axiom the whole dedup design stands on. -/
axiom address_deterministic (b : Bytes) : address b = address b

/-- Injectivity **assumption**, not a theorem: BLAKE3 injectivity is
computationally assumed, never provable. Every claim that needs it cites
this axiom honestly. -/
axiom address_injective : ∀ a b : Bytes, address a = address b → a = b

/-- The store: committed objects as an association of address → bytes. -/
def Store := List (Nat × Bytes)

/-- Whether the store holds an address. -/
def holds (store : Store) (addr : Nat) : Bool :=
  store.any fun p => p.1 == addr

/-- Ingest: `INSERT OR IGNORE` semantics — a second ingest of identical
bytes changes nothing, by construction. -/
def ingest : Store → Bytes → Store
  | store, b =>
      let addr := address b
      if holds store addr then store else store ++ [(addr, b)]

/-- **Theorem (dedup by construction).** Ingesting the same bytes twice
leaves the store unchanged after the first. This is the machine-checked
form of the CAS headline claim: `newly_written == false` on the second
ingest. -/
theorem ingest_is_idempotent (store : Store) (b : Bytes) :
    ingest (ingest store b) b = ingest store b := by
  unfold ingest
  by_cases h : holds store (address b) = true
  · simp [h]
  · simp only [h, if_false]
    have h' : holds store (address b) = false := by
      simpa [holds] using h
    simp [h', ingest]

/-- **Theorem (different bytes, different addresses).** Under the
injectivity assumption, different bytes claim different addresses. The
honest reading: this holds *modulo* the computational assumption; a
SHA-1-style break would void it — which is why YP-CRYPTO-SHA1-003 keeps
BLAKE3 as the only identity and demotes SHA-1 to a hint that can be
wrong without hurting anyone. -/
theorem different_bytes_different_addresses (a b : Bytes) (hne : a ≠ b) :
    address a ≠ address b := fun h => hne (address_injective a b h)

/-- The garbage collector: a *marked set* of addresses the index still
wants, and a sweep that keeps exactly the marked objects. Mirrors
`Pipeline::gc` (YP-CAS-GC-002). -/
def gcSweep (store : Store) (marked : Nat → Bool) : Store :=
  store.filter fun p => marked p.1

/-- **Theorem (GC safety).** Sweeping keeps every marked object. This is
the rule the implementation must satisfy: after `Pipeline::gc`, every
address the index holds resolves to bytes on disk. -/
theorem gc_safety (store : Store) (marked : Nat → Bool) :
    ∀ addr, marked addr = true → holds store addr = true →
      holds (gcSweep store marked) addr = true := by
  intro addr hm held
  unfold gcSweep holds at *
  induction store with
  | nil => simpa using held
  | cons head tail ih =>
      simp only [List.any_cons] at held
      by_cases hkeep : marked head.1 = true
      · rw [List.filter_cons_of_pos (by simpa using hkeep)]
        simp only [List.any_cons, Bool.or_eq_true]
        by_cases hhit : (head.1 == addr) = true
        · simp [hhit]
        · exact ih (by simpa [hhit] using held)
      · rw [List.filter_cons_of_neg (by simpa using hkeep)]
        simp only [List.any_cons, Bool.or_eq_true] at held ⊢
        by_cases hhit : (head.1 == addr) = true
        · exact absurd hhit held
        · exact ih held

end Aperture.Cas
