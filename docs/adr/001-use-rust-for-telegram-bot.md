# ADR-001: Use Rust for Telegram Bot Implementation

## Context
The bot handles heavy multimedia processing (YouTube download, ONNX model inference, Ghostscript PDF compression, audio denoise/separation) alongside a high-throughput Telegram messaging flow.

## Decision
We chose **Rust** (`frankenstein` crate + Tokio) over Python/Node.js to ensure maximum execution speed, low memory footprint, and compile-time concurrency safety.

## Consequences
- **Positive:** Zero runtime crashes from undefined properties/null pointers, minimal CPU overhead.
- **Negative:** Longer compilation time, requires explicit type management and memory handling.
