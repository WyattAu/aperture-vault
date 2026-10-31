---
document_id: YP-CRYPTO-ATREST-004
version: 1.0.0
status: APPROVED (refusal, documented)
domain: Cryptography / Encryption at Rest
subdomains: [Full-Disk vs Application-Layer, Age, Repo Encryption]
created: 2026-10-15
author: Loop
proof_files: []
confidence_level: 0.94
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: At-Rest Encryption: Refused in Favor of the Layer That Already Exists

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| restic paper (Polze et al. reconciliations; restic *Design* doc) | restic encrypts the repo wholesale: chunk → AES-256-CTR + Poly1305 MAC, keys wrapped per host. Backup targets are untrusted by definition, so the crypto is the product. |
| age design (Valsorda, 2019) | Modern file encryption: X25519 recipients, ChaCha20-Poly1305 payloads, tiny spec. The right tool *if* application-layer file encryption is wanted. |
| Cryptomator / gocryptfs write-ups | Overlay filesystems encrypt directories transparently; the app sees plaintext. Latency and inode overhead are the costs. |
| Immich & PhotoPrism | **No at-rest encryption.** Both assume the storage layer (or LUKS) handles it. The photo-server state of the art does not encrypt. |
| LUKS/dm-crypt (the estate's fleet standard) | Full-disk encryption below the filesystem: zero application complexity, protects against physical theft of the disk. |

## YP-2: The threat model decides

Whom is encryption protecting the photos from?

- **Stolen disk / decommissioned drive:** LUKS on the host solves it
  completely, for every application at once. Application-layer
  encryption adds nothing LUKS does not already do here.
- **Remote object storage (B2/S3):** real exposure; and the moment the
  vault supports an S3 backend, application-layer encryption (age) becomes
  mandatory, because the remote bucket is untrusted.
- **Compromised running server:** neither LUKS nor app-layer encryption
  helps; keys are in memory.

## YP-3: Decision

Refused for local storage; the estate's LUKS layer is the chosen
answer, and application-layer encryption would add key management
(key rotation, lost-key recovery, share-link interplay) without
removing any threat LUKS leaves open. **Conditionally mandatory in the
future:** the day a remote object-storage backend ships, age-encrypted
objects (restic's shape) are the design, and this paper is the pointer
back.

## YP-4: Comparison verdict

| System | At rest | Why |
|---|---|---|
| restic / kopia | encrypted | Untrusted targets are the product |
| Immich / PhotoPrism | plaintext | Trusted local storage assumed |
| **aperture-vault** | plaintext + LUKS posture | Same assumption as Immich, stated explicitly |

The refusal is written down so the day the assumption breaks (remote
storage), the spec change is a pointer, not a debate.
