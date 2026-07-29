# ADR 005: Prometheus Metrics & Health Check Server

## Context
Production monitoring requires real-time quantitative metrics (request throughput, latency histograms, active download counters, error frequencies) and standard health probes.

## Decision
Implement a lightweight HTTP server on port `14380` serving:
- `/health` with JSON health/readiness status (`mark_healthy()`, `mark_ready()`).
- `/metrics` exposing standard Prometheus text-format metrics.
- Utilize RAII guards (`ActiveDownloadGuard`, `RequestDurationGuard`) for automatic increment/decrement and duration tracking.

## Consequences
- Enables standard Prometheus / Grafana observability scraping.
- RAII guards prevent metric leaks even on panic or early return.
