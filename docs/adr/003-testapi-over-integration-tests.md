# ADR-003: Internal TestAPI Server for Offline Feature Verification

## Context
Testing Telegram bot features traditionally requires round-tripping through Telegram servers or maintaining complex userbot infrastructure.

## Decision
We implement a local `testapi` HTTP server (`src/testapi/`, gated via `cfg(feature = "testapi")`) on port `14379` that executes real production handlers directly and captures output structures.

## Consequences
- **Positive:** Fast, offline, deterministic local verification for all bot features without Telegram network dependency.
- **Negative:** Feature handlers must expose testable entrypoints and capture traces when testapi feature is active.
