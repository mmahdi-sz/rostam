# ADR 007: Graceful Shutdown with Active Task Draining

## Context
Abruptly terminating the bot during active media downloads, ffmpeg processing, or database transactions risks file corruption and orphan downloads.

## Decision
- Maintain a global atomic task counter `ACTIVE_TASKS` tracking in-flight operations (`TaskGuard`).
- Listen for `SIGINT` / `SIGTERM` signals.
- On shutdown signal, stop receiving new updates, mark `/health` as unready, and drain active tasks up to a 30-second timeout before exiting.

## Consequences
- Ensures zero dropped downloads or corrupted state during systemd restarts or deployments.
