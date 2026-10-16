/-
  Lean4 spec model: share scoping, trash exclusion, checksum ambiguity
  Component: aperture-server, aperture-index
  Yellow Papers: YP-SHARE-SCOPING-007, YP-TRASH-VISIBILITY-008,
                 YP-CRYPTO-SHA1-003

  Mirrors crates/server/src/immich.rs (authorize_asset, the columnar
  timeline, bulk-upload-check) and crates/index (trashed_at filters).
-/

namespace Aperture.Policy

/-! ## Share scoping -/

/-- A share key names one album. -/
abbrev Key := String
abbrev AssetId := String
abbrev AlbumId := String

/-- The album a key grants access to, if the key is live. -/
axiom keyAlbum : Key → Option AlbumId

/-- Album membership, as the index holds it. -/
axiom member : AlbumId → AssetId → Bool

/-- **Authorization rule.** A session grants everything (modelled: always
authorized); a share key grants exactly the members of its album.
Mirrors `authorize_asset`. -/
def authorize (sessionValid : Bool) (keyAlbumOpt : Option AlbumId) (asset : AssetId) : Bool :=
  sessionValid || (keyAlbumOpt.isSome && member keyAlbumOpt.get asset)

/-- **Theorem (share scoping).** If a share key (no session) authorizes
an asset, that asset is a member of the key's album. This is the
machine-checked form of "a share key reaches exactly its album's
members" — the guarantee the share-link arc pinned in tests. -/
theorem share_scoping (key : Key) (asset : AssetId) :
    authorize false (keyAlbum key) asset = true →
    member (keyAlbum key).get asset = true := by
  intro hauth
  unfold authorize at hauth
  by_cases hsome : keyAlbum key |>.isSome
  · simpa [hsome] using hauth
  · simp [hsome] at hauth

/-! ## Trash exclusion -/

/-- An asset is visible on a read path iff it exists and is not trashed.
Mirrors every `WHERE trashed_at IS NULL` the index carries. -/
def visible (present : AssetId → Bool) (trashedAt : AssetId → Option String)
    (asset : AssetId) : Bool :=
  present asset && !(trashedAt asset).isSome

/-- **Theorem (trash exclusion).** A trashed asset is not visible — on
any read path that uses the rule. One predicate, every read. -/
theorem trashed_is_invisible (present trashedAt : AssetId → Option String)
    (asset : AssetId) (h : trashedAt asset |>.isSome) :
    visible exists_ trashedAt asset = false := by
  unfold visible
  simp [h]

/-! ## Checksum ambiguity -/

/-- The lookup the bulk-upload-check performs: all assets carrying a
SHA-1. The ambiguity case is real — a SHA-1 collision means two
different assets legitimately share one checksum. -/
axiom assetsBySha1 : String → List AssetId

/-- The answer rule: exactly one match ⇒ duplicate (skip the upload);
anything else — none, or several — ⇒ accept. The *several* case is the
honest hardening: under a SHA-1 collision the vault refuses to claim
knowledge it cannot have, and the upload itself (BLAKE3-addressed)
resolves identity. Mirrors `bulk_upload_check`. -/
noncomputable def answer (checksum : String) : String :=
  match assetsBySha1 checksum with
  | [single] => s!"duplicate:{single}"
  | _ => "accept"

/-- **Theorem (ambiguity is never a skip).** When two or more assets
share a checksum, the answer is *accept* — the vault never tells a
client "already have it" on the strength of a collision-prone hash
alone. -/
theorem ambiguous_never_skips (checksum : String)
    (many : (assetsBySha1 checksum).length ≥ 2) :
    answer checksum = "accept" := by
  unfold answer
  match h : assetsBySha1 checksum with
  | [] => rfl
  | [single] =>
      exfalso
      have := h ▸ many
      simp at this
  | first :: second :: rest => rfl

end Aperture.Policy
