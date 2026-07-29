# ADR 004: Structured & Trace-Correlated Logging

## Context
When handling high volumes of async user interactions across YouTube downloads, STT, and media processing, tracing failures required correlated logs across step boundaries.

## Decision
Adopt a strict single-`trace_id` per action standard using structured domain prefixes:
- Entry handler logs `[<domain> trace=N actor] user=@<u> id=<id> rank=<R> clicked=<cb>`
- Internal steps log `[<domain> trace=N event=<step>] k=v => <outcome>`

## Consequences
- Every user interaction can be completely replayed in sequence via `journalctl -u abc | rg trace=N`.
- Zero raw `eprintln!` calls for traces; macro enforcement via `log_actor!` and `log_ev!`.
