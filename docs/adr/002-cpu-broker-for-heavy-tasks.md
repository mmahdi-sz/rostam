# ADR-002: CPU Broker Architecture for Heavy Tasks

## Context
CPU-heavy tasks (e.g. Moebius ONNX watermark removal, vocal separation, audio denoise) can lock system resources if run directly on the async event loop.

## Decision
All operations taking >500ms CPU time MUST route through the **CPU Broker** (Redis core reservation on port `:6589`).

## Consequences
- **Positive:** System remains responsive under heavy concurrent load; server never hangs or panics from OOM.
- **Negative:** Handlers must yield and wait for CPU allocation.
