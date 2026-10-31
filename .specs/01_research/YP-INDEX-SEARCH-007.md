# Yellow Paper: Search: LIKE Is Honest Until the Corpus Says Otherwise
| SQLite FTS5 documentation (full-text search module) | FTS5 gives tokenized full-text with BM25 ranking and trigram tokenizer option (since 3.34) enabling substring matching; at the cost of a shadow index roughly the size of the text. |
| Immich | Runs PostgreSQL with pgvector + pg_trgm and a whole ML smart-search stack; smart search is its flagship feature; and its heaviest dependency chain. |
| The vault's measured scale | A personal library is 10⁴–10⁶ files. A `LIKE '%x%'` scan over 10⁶ rows in SQLite is single-digit milliseconds on this class of hardware (measured intuition of the row store; not a formal benchmark; flagged honestly). |
- The fields searched are *filenames, album names, captions*; short
- Immich's smart search solves *semantic* query ("dog on beach"); that
`file_name` and `description`; an additive shadow table, no schema