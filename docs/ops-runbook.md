# Operations & Incident Runbook

## Overview
This document describes operational procedures, monitoring endpoints, incident response, and rollback guidelines for `ros-telegram-bot`.

## Monitoring & Health Checks
- **Health Endpoint:** `GET http://127.0.0.1:14380/health`
  - Returns `200 OK` with JSON `{"healthy": true, "ready": true}` when operational.
  - Returns `503 Service Unavailable` if unready or shutting down.
- **Prometheus Metrics:** `GET http://127.0.0.1:14380/metrics`
  - `bot_requests_total{feature, status}`
  - `bot_request_duration_seconds{feature}`
  - `bot_active_downloads`
  - `bot_errors_total{feature}`

## Systemd Operations
- **Status:** `sudo systemctl status abc.service`
- **Restart:** `sudo systemctl restart abc.service`
- **Logs:** `journalctl -u abc.service -f -n 100`
- **Trace Replay:** `journalctl -u abc.service -n 300 | rg trace=<TRACE_ID>`

## Incident Response & Troubleshooting
1. **Service Fails to Start:**
   - Verify `BOT_TOKEN` in `.env` or `/etc/default/abc`.
   - Ensure PostgreSQL database `ros_telegram_bot` is accessible.
   - Check local Bot API server is running on `BOT_API_BASE_URL`.
2. **High CPU / Memory Usage:**
   - Check active downloads via Prometheus metric `bot_active_downloads`.
   - Verify ONNX/ffmpeg processes via `ps aux | grep ros`.
3. **Rollback Procedure:**
   - Dev environment: `git checkout HEAD^` in `/mnt/data/mahdidev/ros/dev` and restart `abc.service`.
   - Production environment: Use deploy script `/mnt/data/mahdidev/ros/deploy.sh` or revert `master` branch.
