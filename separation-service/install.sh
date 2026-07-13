#!/usr/bin/env bash
#
# One-shot installer for the Vocal/Instrumental Separation Service.
#
# Bootstraps EVERYTHING on a bare server (Debian/Ubuntu *or* Arch):
#   - system prerequisites (python, ffmpeg, redis, build tools, curl, git)
#   - Redis service (enable + start, correct unit name per distro)
#   - Python venv + all requirements
#   - Kim_Vocal_2.onnx model (downloaded if missing)
#   - systemd unit for the service (install + enable + start)
#   - health check on port 6589
#
# Safe to re-run (idempotent). Auto-elevates with sudo if not root.
#
# Usage:
#   sudo bash install.sh              # full install
#   sudo bash install.sh --fresh      # rebuild the venv from scratch
#   sudo bash install.sh --no-service # set up venv+model only, skip systemd
#   bash install.sh --help
#
set -Eeuo pipefail

# ---------------------------------------------------------------------------
# Paths & constants (derived from this script's location — no hardcoding).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # separation-service/
WORKDIR="$(dirname "$SCRIPT_DIR")"                            # repo root (ros/dev)
VENV="$SCRIPT_DIR/venv"
MODELS="$SCRIPT_DIR/models"
MODEL_FILE="$MODELS/Kim_Vocal_2.onnx"
START_SH="$SCRIPT_DIR/start.sh"

SERVICE_NAME="separation"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
PORT=6589
HEALTH_URL="http://127.0.0.1:${PORT}/health"

# Owner of the tree — venv & model stay owned by them, service runs as root.
RUN_USER="$(stat -c '%U' "$SCRIPT_DIR")"

FRESH=0
INSTALL_SERVICE=1

# ---------------------------------------------------------------------------
# Logging helpers.
# ---------------------------------------------------------------------------
if [ -t 1 ]; then C_B=$'\033[1m'; C_G=$'\033[32m'; C_Y=$'\033[33m'; C_R=$'\033[31m'; C_0=$'\033[0m'
else C_B=; C_G=; C_Y=; C_R=; C_0=; fi
say()  { printf '%s[install]%s %s\n' "$C_B" "$C_0" "$*"; }
ok()   { printf '%s[install] ✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '%s[install] !%s %s\n' "$C_Y" "$C_0" "$*" >&2; }
die()  { printf '%s[install] ✗%s %s\n' "$C_R" "$C_0" "$*" >&2; exit 1; }
trap 'die "failed at line $LINENO (exit $?)"' ERR

# ---------------------------------------------------------------------------
# Arg parsing.
# ---------------------------------------------------------------------------
for arg in "$@"; do
  case "$arg" in
    --fresh)      FRESH=1 ;;
    --no-service) INSTALL_SERVICE=0 ;;
    -h|--help)
      sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) die "unknown option: $arg (try --help)" ;;
  esac
done

# ---------------------------------------------------------------------------
# Re-exec as root if needed (packages + systemd require it).
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
  say "elevating with sudo…"
  exec sudo -E bash "${BASH_SOURCE[0]}" "$@"
fi

# Run a command as the tree owner (so files aren't root-owned in a user dir).
run_as() {
  if [ "$RUN_USER" != "root" ]; then
    sudo -H -u "$RUN_USER" "$@"
  else
    "$@"
  fi
}

# ---------------------------------------------------------------------------
# 1. Detect distro / package manager.
# ---------------------------------------------------------------------------
. /etc/os-release 2>/dev/null || die "cannot read /etc/os-release"
DISTRO_FAMILY=""
case "${ID:-}:${ID_LIKE:-}" in
  *arch*)                 DISTRO_FAMILY="arch" ;;
  *debian*|*ubuntu*)      DISTRO_FAMILY="debian" ;;
  *) die "unsupported distro: ID=${ID:-?} ID_LIKE=${ID_LIKE:-?} (only Arch & Debian/Ubuntu)" ;;
esac
say "distro=${PRETTY_NAME:-$ID} family=$DISTRO_FAMILY owner=$RUN_USER"

# ---------------------------------------------------------------------------
# 2. Install system prerequisites.
# ---------------------------------------------------------------------------
say "installing system packages…"
if [ "$DISTRO_FAMILY" = "debian" ]; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y --no-install-recommends \
    python3 python3-venv python3-pip python3-dev \
    ffmpeg redis-server build-essential \
    curl ca-certificates git
else # arch
  pacman -Sy --noconfirm --needed \
    python python-pip \
    ffmpeg redis base-devel \
    curl ca-certificates git
fi
ok "system packages ready"

# ---------------------------------------------------------------------------
# 3. Enable + start Redis (unit name differs per distro/package).
# ---------------------------------------------------------------------------
say "configuring Redis…"
# Capture the unit list once, then match against it. Piping systemctl directly
# into `grep -q` breaks under `set -o pipefail`: grep exits on first match, the
# resulting SIGPIPE kills systemctl with code 141, and pipefail fails the `if`.
REDIS_UNIT=""
UNIT_FILES="$(systemctl list-unit-files --type=service 2>/dev/null || true)"
for u in redis-server redis valkey; do
  if grep -q "^${u}\.service" <<<"$UNIT_FILES"; then
    REDIS_UNIT="$u"; break
  fi
done
[ -n "$REDIS_UNIT" ] || die "no redis/valkey systemd unit found after install"
systemctl enable --now "$REDIS_UNIT" >/dev/null 2>&1 || systemctl restart "$REDIS_UNIT"
if redis-cli ping 2>/dev/null | grep -qi pong; then
  ok "Redis up (unit=$REDIS_UNIT)"
else
  warn "Redis unit '$REDIS_UNIT' started but PING did not answer — check: systemctl status $REDIS_UNIT"
fi

# ---------------------------------------------------------------------------
# 4. Python venv + requirements.
# ---------------------------------------------------------------------------
PYBIN="$(command -v python3 || command -v python)" || die "python not found"

if [ "$FRESH" = "1" ] && [ -d "$VENV" ]; then
  say "--fresh: removing existing venv"
  rm -rf "$VENV"
fi

if [ ! -x "$VENV/bin/python3" ] && [ ! -x "$VENV/bin/python" ]; then
  say "creating venv ($PYBIN)…"
  run_as "$PYBIN" -m venv "$VENV"
else
  say "venv exists — reusing"
fi

PIP="$VENV/bin/pip"
say "installing Python requirements (this can take a while)…"
run_as "$PIP" install --upgrade pip
run_as "$PIP" install wheel setuptools
run_as "$PIP" install -r "$SCRIPT_DIR/requirements.txt"
ok "Python dependencies installed"

# ---------------------------------------------------------------------------
# 5. Pre-download the model (skip if already present).
# ---------------------------------------------------------------------------
mkdir -p "$MODELS"; chown "$RUN_USER" "$MODELS" 2>/dev/null || true
if [ -f "$MODEL_FILE" ] && [ "$(stat -c '%s' "$MODEL_FILE")" -gt 1000000 ]; then
  ok "model already present ($(du -h "$MODEL_FILE" | cut -f1)) — skipping download"
else
  say "downloading Kim_Vocal_2.onnx…"
  run_as "$VENV/bin/python3" - "$MODELS" <<'PY'
import sys
from audio_separator.separator import Separator
s = Separator(model_file_dir=sys.argv[1], log_level=20)
s.load_model('Kim_Vocal_2.onnx')
print('[install] model downloaded')
PY
  ok "model downloaded"
fi

# ---------------------------------------------------------------------------
# 6. systemd service (install + enable + start).
# ---------------------------------------------------------------------------
if [ "$INSTALL_SERVICE" = "1" ]; then
  say "installing systemd unit → $UNIT_PATH"
  chmod +x "$START_SH" 2>/dev/null || true
  cat > "$UNIT_PATH" <<UNIT
[Unit]
Description=Vocal/Instrumental Separation Service
After=network.target ${REDIS_UNIT}.service
Wants=${REDIS_UNIT}.service

[Service]
Type=simple
WorkingDirectory=${WORKDIR}
ExecStart=${START_SH}
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable "$SERVICE_NAME" >/dev/null 2>&1 || true
  systemctl restart "$SERVICE_NAME"
  ok "service ${SERVICE_NAME}.service enabled and (re)started"

  # -------------------------------------------------------------------------
  # 7. Health check (model loads in a background thread, so retry a bit).
  # -------------------------------------------------------------------------
  say "waiting for the service on port ${PORT}…"
  healthy=0
  for i in $(seq 1 30); do
    if body="$(curl -fsS "$HEALTH_URL" 2>/dev/null)"; then
      healthy=1
      break
    fi
    sleep 2
  done
  if [ "$healthy" = "1" ]; then
    ok "service is up → $body"
    case "$body" in
      *'"model_loaded":true'*|*'"model_loaded": true'*)
        ok "model loaded" ;;
      *) warn "service up but model still loading — first request may wait a few seconds" ;;
    esac
  else
    warn "no health response yet. Inspect with: journalctl -u ${SERVICE_NAME} -n 100 --no-pager"
  fi
else
  say "--no-service: skipping systemd install. Run manually with: $START_SH"
fi

echo
ok "Done. Health: curl $HEALTH_URL"
[ "$INSTALL_SERVICE" = "1" ] && say "Logs: journalctl -u ${SERVICE_NAME} -f"
exit 0
