---
document_id: YP-SERVER-CONCURRENCY-010
version: 1.0.0
status: APPROVED (refusals documented, gap closed)
domain: Server Systems / Concurrency and Transport
subdomains: [io_uring, Tokio, Blocking Pools, Endpoint Hygiene]
created: 2026-10-15
author: Loop
proof_files: []
confidence_level: 0.96
tqa_level: 3
research_iterations: 2
---

# Yellow Paper: Concurrency — Tokio Retained, io_uring Refused (Twice), the Blocking Rule Made Load-Bearing

## YP-1: Research Basis

| Source | What it establishes |
|---|---|
| *Scalable IO multiplexing* lineage (C10K and successors; epoll literature) | The readiness model (tokio/epoll) serves 10⁴+ concurrent connections; kernel-bypass (io_uring) wins on syscall *entry* cost and deep queue depth. |
| *Racing the Storage Daemon* / io_uring server experiments (2021–2024 write-ups, e.g. Boost.Asio io_uring expositions) | io_uring servers beat epoll when workloads are *many small I/Os per connection* (proxies, caches). For few-connections, payload-heavy transfers, the win vanishes into the copy cost. |
| YP-CAS-GC-002's sibling measurement (`examples/ingest_profile.rs`) | The vault's ingest path: syscall share 0.0045%. The file-I/O refusal stands. |
| Immich's own stack | NestJS/Node microservices + PostgreSQL + Redis + ML workers — a concurrency story built for *scale*, which a single-user home server does not have. Its known pain: memory. The vault's single axum process is the counter-design. |
| tokio documentation: `spawn_blocking` contract | Blocking work on the runtime starves other tasks unless moved to the blocking pool — the rule the thumbnail/scan handlers already follow. |

## YP-2: The decisions, held and hardened

1. **io_uring for file I/O: refused (measured).** YP-CAS-GC-002's sibling
   measurement stands; nothing in this iteration reopens it.
2. **io_uring for sockets: refused (unmeasured promise).** The vault's
   connection concurrency is household-scale; uring-kit remains the
   estate's socket substrate for shallow-poll edge serving (vane), and
   the day the vault needs it, the integration point is documented
   (replace `axum::serve`'s acceptor, not the handlers).
3. **The blocking rule made structural:** every CPU-bound or std-IO
   handler (thumbnail derivation) must ride `spawn_blocking`. This was
   a convention; the YP makes it a review invariant, and the handler
   structure (state cloned Arcs in, guard held inside) is the pattern
   to copy.
4. **Endpoint hygiene from this loop's findings:** one URL per resource
   (the upload dispatcher), errors are uniform, and per-request leaks
   (the `Box::leak` header value caught and fixed in the download
   archive arc) are the class of bug a long-running server cannot
   afford.

## YP-3: Spec consequence

No transport change. The concurrency spec is: axum/tokio for sockets;
`spawn_blocking` for decode/scan work; std I/O for CAS files; the
2 GiB in-memory archive cap; one blocking rule, load-bearing in code
review. The refusals are recorded with their measurements so the next
"modernize the transport" idea reads the numbers first.
