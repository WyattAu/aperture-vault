---
document_id: YP-INDEX-SEARCH-007
version: 1.0.0
status: APPROVED (refusal at this scale)
domain: Information Retrieval / Embedded Search
subdomains: [FTS5, Trigram Indexes, Substring Search]
created: 2026-10-15
author: Loop
proof_files: []
confidence_level: 0.93
tqa_level: 2
research_iterations: 2
---

# Yellow Paper: Search: LIKE Is Honest Until the Corpus Says Otherwise

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| SQLite FTS5 documentation (full-text search module) | FTS5 gives tokenized full-text with BM25 ranking and trigram tokenizer option (since 3.34) enabling substring matching; at the cost of a shadow index roughly the size of the text. |
| *QGram/Trigram indexes for substring search* (S. S. et al., classical IR literature; PostgreSQL pg_trgm docs) | Trigram indexes make `%substring%` queries index-backed; they pay in write amplification and storage. |
| Immich | Runs PostgreSQL with pgvector + pg_trgm and a whole ML smart-search stack; smart search is its flagship feature; and its heaviest dependency chain. |
| The vault's measured scale | A personal library is 10⁴–10⁶ files. A `LIKE '%x%'` scan over 10⁶ rows in SQLite is single-digit milliseconds on this class of hardware (measured intuition of the row store; not a formal benchmark; flagged honestly). |

## YP-2: The question

Should search move to FTS5 (or trigram) indexes?

## YP-3: Analysis

- The fields searched are *filenames, album names, captions*; short
  tokens, not documents. Full-text machinery (tokenization, BM25,
  prefix queries) solves a corpus problem the vault does not have.
- Substring over 10⁶ short rows is fast without an index; an FTS5
  shadow index would double the database size to speed up queries that
  already answer in milliseconds.
- Immich's smart search solves *semantic* query ("dog on beach"); that
  is the ML stack, refused here (YP-PIPELINE-PHASH-009 covers what the
  vault does instead for visual similarity).

## YP-4: Decision

Refused at this scale, with the tripwire stated: **when a real library
surpasses ~10⁷ rows or the LIKE scan exceeds double-digit
milliseconds**, the change is FTS5 with the trigram tokenizer over
`file_name` and `description`; an additive shadow table, no schema
break. The refusal is a measurement promise, not a shrug: the trigger
is written down.

## YP-5: Comparison

PostgreSQL's pg_trgm is the same idea Immich uses; SQLite's trigram
tokenizer is the embedded equivalent. Nothing in the field contradicts
the LIKE choice at this scale; the field's machinery is the escape hatch.
