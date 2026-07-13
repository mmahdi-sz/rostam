#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR="/opt/rostam"
REPO="${ROSTAM_REPO:-https://github.com/mmahdi-sz/rostam.git}"
BRANCH="${ROSTAM_BRANCH:-master}"
DENO_DIR="/opt/deno"
ASR_MODEL_DIR="/opt/asr_model"
BOTAPI_PORT=8081

FRESH=0
SKIP_BOTAPI=0
SKIP_ASR=0
SKIP_FIREFOX=0

if [ -t 1 ]; then C_B=$'\033[1m'; C_G=$'\033[32m'; C_Y=$'\033[33m'; C_R=$'\033[31m'; C_0=$'\033[0m'
else C_B=; C_G=; C_Y=; C_R=; C_0=; fi
say()  { printf '%s[rostam]%s %s\n' "$C_B" "$C_0" "$*"; }
ok()   { printf '%s[rostam] ✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '%s[rostam] !%s %s\n' "$C_Y" "$C_0" "$*" >&2; }
die()  { printf '%s[rostam] ✗%s %s\n' "$C_R" "$C_0" "$*" >&2; exit 1; }
trap 'die "failed at line $LINENO (exit $?)"' ERR

usage() {
  cat <<'EOF'
One-shot installer for the rostam Telegram bot (Rust + Python sidecars).

Bootstraps EVERYTHING on a bare Debian/Ubuntu or Arch server: clones the repo,
installs system packages + Rust + deno + yt-dlp, downloads ~4.2 GB of models,
sets up PostgreSQL + Redis, builds the bot, and installs every sidecar
(separation :6589, ASR :8765, local Telegram Bot API :8081) as systemd services.

Usage:
  bash <(curl -Ls https://raw.githubusercontent.com/mmahdi-sz/rostam/master/install.sh)
  sudo bash install.sh [options]

Options:
  --dir <path>      install location (default /opt/rostam)
  --branch <name>   git branch to clone (default master)
  --skip-bot-api    do not build/install the local Telegram Bot API server
  --skip-asr        do not install the ASR service (:8765)
  --skip-firefox    do not install Firefox (cookie-pool refresher)
  --fresh           re-clone / rebuild from scratch
  -h, --help        show this help
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dir)         INSTALL_DIR="$2"; shift 2 ;;
    --branch)      BRANCH="$2"; shift 2 ;;
    --skip-bot-api) SKIP_BOTAPI=1; shift ;;
    --skip-asr)    SKIP_ASR=1; shift ;;
    --skip-firefox) SKIP_FIREFOX=1; shift ;;
    --fresh)       FRESH=1; shift ;;
    -h|--help)     usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  say "elevating with sudo…"
  exec sudo -E bash "$0" \
    --dir "$INSTALL_DIR" --branch "$BRANCH" \
    $([ "$SKIP_BOTAPI" = 1 ] && echo --skip-bot-api) \
    $([ "$SKIP_ASR" = 1 ] && echo --skip-asr) \
    $([ "$SKIP_FIREFOX" = 1 ] && echo --skip-firefox) \
    $([ "$FRESH" = 1 ] && echo --fresh)
fi

# ─── distro detection ────────────────────────────────────────────────────────
. /etc/os-release 2>/dev/null || die "cannot read /etc/os-release"
case "${ID:-}:${ID_LIKE:-}" in
  *arch*)            FAMILY="arch" ;;
  *debian*|*ubuntu*) FAMILY="debian" ;;
  *) die "unsupported distro: ID=${ID:-?} (only Arch & Debian/Ubuntu)" ;;
esac
say "distro=${PRETTY_NAME:-$ID} family=$FAMILY install_dir=$INSTALL_DIR branch=$BRANCH"

pac() {                       # non-interactive pacman (picks provider defaults)
  local rc=0
  set +o pipefail
  yes '' 2>/dev/null | pacman "$@" || rc=$?
  set -o pipefail
  return "$rc"
}

fetch() { curl -fL --retry 3 --retry-delay 2 -o "$2" "$1"; }   # url dest

# download+size-gate: skip if file exists and is bigger than $3 bytes
need_file() {                 # url dest min_bytes label
  local url="$1" dest="$2" min="$3" label="$4"
  if [ -f "$dest" ] && [ "$(stat -c '%s' "$dest" 2>/dev/null || echo 0)" -gt "$min" ]; then
    ok "$label present — skipping"
  else
    say "downloading $label…"
    mkdir -p "$(dirname "$dest")"
    fetch "$url" "$dest"
    ok "$label downloaded"
  fi
}

# ─── 1. system packages ──────────────────────────────────────────────────────
say "installing system packages…"
if [ "$FAMILY" = "debian" ]; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  PKGS="git curl ca-certificates unzip tar xz-utils build-essential pkg-config \
        ffmpeg ghostscript redis-server postgresql postgresql-client \
        python3 python3-venv python3-pip procps cmake g++ make gperf zlib1g-dev libssl-dev"
  [ "$SKIP_FIREFOX" = 0 ] && PKGS="$PKGS firefox-esr"
  apt-get install -y --no-install-recommends $PKGS
  apt-get install -y --no-install-recommends rar 2>/dev/null || warn "rar unavailable (surge archive-split degrades)"
else
  PKGS="git curl ca-certificates unzip tar xz base-devel ffmpeg ghostscript \
        redis postgresql python python-pip procps cmake gperf"
  [ "$SKIP_FIREFOX" = 0 ] && PKGS="$PKGS firefox"
  pac -Sy --noconfirm --needed archlinux-keyring
  pac -Syu --noconfirm --needed $PKGS
fi
ok "system packages ready"

# Rust (edition 2024 → rustc ≥ 1.85)
if ! command -v cargo >/dev/null 2>&1; then
  say "installing Rust via rustup…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
# shellcheck disable=SC1090
. "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || die "cargo not on PATH after rustup"
ok "rust $(rustc --version | awk '{print $2}')"

# deno (yt-dlp --js-runtimes)
if [ ! -x "$DENO_DIR/bin/deno" ]; then
  say "installing deno → $DENO_DIR…"
  curl -fsSL https://deno.land/install.sh | env DENO_INSTALL="$DENO_DIR" sh -s -- -y >/dev/null 2>&1 || true
fi
[ -x "$DENO_DIR/bin/deno" ] && ok "deno ready ($DENO_DIR/bin/deno)" || warn "deno install failed (YouTube JS challenges degrade)"

# yt-dlp latest static binary
if ! command -v yt-dlp >/dev/null 2>&1; then
  say "installing yt-dlp…"
  fetch "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp" /usr/local/bin/yt-dlp
  chmod +x /usr/local/bin/yt-dlp
fi
ok "yt-dlp ready"

# ─── 2. clone repo ───────────────────────────────────────────────────────────
if [ "$FRESH" = 1 ] && [ -d "$INSTALL_DIR/.git" ]; then
  warn "--fresh: removing $INSTALL_DIR"; rm -rf "$INSTALL_DIR"
fi
if [ -d "$INSTALL_DIR/.git" ]; then
  say "repo exists — fetching latest $BRANCH…"
  git -C "$INSTALL_DIR" fetch --depth 1 origin "$BRANCH"
  git -C "$INSTALL_DIR" checkout "$BRANCH"
  git -C "$INSTALL_DIR" reset --hard "origin/$BRANCH"
else
  say "cloning $REPO ($BRANCH) → $INSTALL_DIR…"
  git clone --branch "$BRANCH" "$REPO" "$INSTALL_DIR"
fi
cd "$INSTALL_DIR"
ok "repo at $(git rev-parse --short HEAD)"

# ─── 3. model / runtime assets (git-ignored, ~4.2 GB) ────────────────────────
say "provisioning model assets (this downloads several GB)…"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"; die "failed at line $LINENO (exit $?)"' ERR

# libvosk.so — extract just the x86_64 shared lib from the release zip
if [ ! -f files/runtime/libvosk.so ]; then
  say "downloading libvosk…"
  fetch "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip" "$TMP/vosk.zip"
  mkdir -p files/runtime
  unzip -o "$TMP/vosk.zip" -d "$TMP/vosk" >/dev/null
  cp "$(find "$TMP/vosk" -name libvosk.so | head -1)" files/runtime/libvosk.so
  ok "libvosk.so"
else ok "libvosk.so present — skipping"; fi

# vosk models (unzip in place; dir names are already versioned)
mkdir -p files/models/vosk
for m in vosk-model-fa-0.42 vosk-model-small-fa-0.42 vosk-model-en-us-0.22-lgraph vosk-model-small-en-us-0.15; do
  if [ -d "files/models/vosk/$m" ]; then ok "vosk/$m present — skipping"; continue; fi
  say "downloading vosk/$m…"
  fetch "https://alphacephei.com/vosk/models/$m.zip" "$TMP/$m.zip"
  unzip -o "$TMP/$m.zip" -d files/models/vosk >/dev/null
  rm -f "$TMP/$m.zip"
  ok "vosk/$m"
done

# moebius onnx (gwm)
need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/unet.onnx"        files/models/moebius/unet.onnx        800000000 "moebius/unet.onnx"
need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/vae_decoder.onnx" files/models/moebius/vae_decoder.onnx 150000000 "moebius/vae_decoder.onnx"
need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/vae_encoder.onnx" files/models/moebius/vae_encoder.onnx 100000000 "moebius/vae_encoder.onnx"

# deepfilter model (kept as tar.gz — code un-tars it) + binary
need_file "https://github.com/Rikorose/DeepFilterNet/raw/1e96ef05e1ef75b3702f8c55ca065368deae637d/models/DeepFilterNet3_onnx.tar.gz" \
          files/models/deepfilter/DeepFilterNet3_onnx.tar.gz 5000000 "deepfilter model"
if [ ! -x files/runtime/deep-filter ]; then
  say "downloading deep-filter binary…"
  if fetch "https://github.com/Rikorose/DeepFilterNet/releases/download/v0.5.6/deep-filter-0.5.6-x86_64-unknown-linux-musl" files/runtime/deep-filter; then
    chmod +x files/runtime/deep-filter; ok "deep-filter binary"
  else warn "deep-filter binary unavailable — STT denoise will degrade"; fi
else ok "deep-filter binary present — skipping"; fi

# real-esrgan (binary + models land flat under files/realesrgan/)
if [ ! -x files/realesrgan/realesrgan-ncnn-vulkan ]; then
  say "downloading Real-ESRGAN…"
  fetch "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.5.0/realesrgan-ncnn-vulkan-20220424-ubuntu.zip" "$TMP/resrgan.zip"
  mkdir -p files/realesrgan
  unzip -o "$TMP/resrgan.zip" -d files/realesrgan >/dev/null
  chmod +x files/realesrgan/realesrgan-ncnn-vulkan
  ok "Real-ESRGAN"
else ok "Real-ESRGAN present — skipping"; fi

rm -rf "$TMP"
trap 'die "failed at line $LINENO (exit $?)"' ERR
ok "assets ready"

# ─── 4. PostgreSQL (DB + role from .env.example DATABASE_URL) ─────────────────
say "configuring PostgreSQL…"
# Arch ships an EMPTY /var/lib/postgres/data with the package — initdb when it's
# empty (not merely absent), else the service won't start.
if [ "$FAMILY" = "arch" ] && [ -z "$(ls -A /var/lib/postgres/data 2>/dev/null)" ]; then
  install -d -o postgres -g postgres /var/lib/postgres/data
  sudo -u postgres initdb --locale=C.UTF-8 -E UTF8 -D /var/lib/postgres/data >/dev/null 2>&1 || true
fi
PG_UNIT="postgresql"
systemctl list-unit-files --type=service 2>/dev/null | grep -q '^postgresql\.service' && PG_UNIT="postgresql" || true
systemctl enable --now "$PG_UNIT" >/dev/null 2>&1 || systemctl restart "$PG_UNIT" || warn "could not start $PG_UNIT"
sleep 2
sudo -u postgres psql -v ON_ERROR_STOP=1 <<'SQL' >/dev/null 2>&1 && ok "DB ros_telegram_bot ready" || warn "postgres provisioning skipped (already set up or unreachable)"
ALTER USER postgres WITH PASSWORD 'postgres';
SELECT 'CREATE DATABASE ros_telegram_bot'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'ros_telegram_bot')\gexec
SQL

# ─── 5. Redis ────────────────────────────────────────────────────────────────
say "configuring Redis…"
REDIS_UNIT=""
UNIT_FILES="$(systemctl list-unit-files --type=service 2>/dev/null || true)"
for u in redis-server redis valkey; do
  grep -q "^${u}\.service" <<<"$UNIT_FILES" && { REDIS_UNIT="$u"; break; }
done
[ -n "$REDIS_UNIT" ] || die "no redis/valkey unit after install"
systemctl enable --now "$REDIS_UNIT" >/dev/null 2>&1 || systemctl restart "$REDIS_UNIT"
redis-cli ping 2>/dev/null | grep -qi pong && ok "Redis up (unit=$REDIS_UNIT)" || warn "redis PING failed"

# ─── 6. .env (prompt for secrets) ────────────────────────────────────────────
if [ ! -f .env ]; then
  say "creating .env from .env.example…"
  cp .env.example .env
  BOT_TOKEN=""
  while [ -z "$BOT_TOKEN" ]; do read -r -p "  BOT_TOKEN (from @BotFather): " BOT_TOKEN; done
  read -r -p "  ADMIN_USER_ID (your Telegram numeric id, optional): " ADMIN_ID
  set_env() { local k="$1" v="$2"; if grep -q "^$k=" .env; then sed -i "s|^$k=.*|$k=$v|" .env; else printf '%s=%s\n' "$k" "$v" >>.env; fi; }
  set_env BOT_TOKEN "$BOT_TOKEN"
  [ -n "${ADMIN_ID:-}" ] && set_env ADMIN_USER_ID "$ADMIN_ID"
  set_env DENO_PATH "$DENO_DIR/bin/deno"
  set_env DATABASE_URL "postgres://postgres:postgres@localhost:5432/ros_telegram_bot"
  [ "$SKIP_BOTAPI" = 1 ] && set_env BOT_API_BASE_URL "" || set_env BOT_API_BASE_URL "http://127.0.0.1:$BOTAPI_PORT"
  chmod 600 .env
  ok ".env written"
else
  ok ".env exists — leaving as-is"
fi

# ─── 7. build the bot ────────────────────────────────────────────────────────
say "building the bot (cargo build --release — this takes a while)…"
cargo build --release
[ -x target/release/ros-telegram-bot ] || die "bot binary missing after build"
ok "bot built"

# Sidecars are best-effort: a failure here (e.g. a pinned dep with no wheel for
# this distro's Python) must NOT abort the install — the core bot is already
# built and still gets its systemd service below.

# ─── 8. separation-service (reuse its tested installer) ──────────────────────
say "installing separation-service (:6589)…"
bash "$INSTALL_DIR/separation-service/setup.sh" || warn "separation-service install failed (:6589 degrades)"

# ─── 9. ASR service (:8765) ──────────────────────────────────────────────────
asr_install() {
  ( cd asr-service
    [ -x venv/bin/python3 ] || python3 -m venv venv
    venv/bin/pip install --upgrade pip >/dev/null
    venv/bin/pip install -r requirements.txt
    mkdir -p "$ASR_MODEL_DIR"
    if [ -f "$ASR_MODEL_DIR/tokenizer.json" ]; then ok "ASR model present — skipping"; else
      say "downloading ASR model (~1.5 GB)…"
      ASR_MODEL_DIR="$ASR_MODEL_DIR" venv/bin/python download_model.py
    fi
  ) || return 1
  cat > /etc/systemd/system/asr.service <<UNIT
[Unit]
Description=Nemotron ASR Service
After=network.target

[Service]
WorkingDirectory=$INSTALL_DIR/asr-service
ExecStart=$INSTALL_DIR/asr-service/venv/bin/uvicorn asr_service:app --host 127.0.0.1 --port 8765 --workers 1
Restart=always
RestartSec=5
Environment=ASR_MODEL_DIR=$ASR_MODEL_DIR
Environment=SEPARATION_SERVICE_DIR=$INSTALL_DIR/separation-service

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now asr.service >/dev/null 2>&1 || systemctl restart asr.service
}
if [ "$SKIP_ASR" = 0 ]; then
  say "installing ASR service (:8765)…"
  if asr_install; then ok "asr.service installed"
  else warn "ASR install failed (:8765 unavailable) — likely a pinned dep with no wheel for this Python; core bot unaffected"; fi
else warn "--skip-asr: ASR service not installed"; fi

# ─── 10. local Telegram Bot API (:8081) ──────────────────────────────────────
botapi_install() {
  if command -v telegram-bot-api >/dev/null 2>&1; then
    ok "telegram-bot-api already installed"; return 0
  fi
  say "building telegram-bot-api from source (heavy: tdlib compile)…"
  read -r -p "  Telegram api_id (my.telegram.org): " TG_API_ID
  read -r -p "  Telegram api_hash: " TG_API_HASH
  local BT; BT="$(mktemp -d)"
  git clone --recursive --depth 1 https://github.com/tdlib/telegram-bot-api.git "$BT/src" || { rm -rf "$BT"; return 1; }
  cmake -S "$BT/src" -B "$BT/build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local || { rm -rf "$BT"; return 1; }
  cmake --build "$BT/build" --target install -j "$(nproc)" || { rm -rf "$BT"; return 1; }
  rm -rf "$BT"
  cat > /etc/systemd/system/telegram-bot-api.service <<UNIT
[Unit]
Description=Local Telegram Bot API server
After=network.target

[Service]
ExecStart=/usr/local/bin/telegram-bot-api --local --api-id=$TG_API_ID --api-hash=$TG_API_HASH --http-port=$BOTAPI_PORT
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now telegram-bot-api.service
  ok "telegram-bot-api installed (:$BOTAPI_PORT)"
}
if [ "$SKIP_BOTAPI" = 0 ]; then
  botapi_install || warn "telegram-bot-api install failed (:8081 unavailable) — bot falls back to official API (20 MB cap); core bot unaffected"
else warn "--skip-bot-api: using official Telegram API (20 MB upload cap)"; fi

# ─── 11. rostam.service ──────────────────────────────────────────────────────
say "installing rostam.service…"
cat > /etc/systemd/system/rostam.service <<UNIT
[Unit]
Description=rostam Telegram Bot
After=network-online.target postgresql.service $REDIS_UNIT.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
EnvironmentFile=-$INSTALL_DIR/.env
ExecStart=$INSTALL_DIR/target/release/ros-telegram-bot
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable rostam.service >/dev/null 2>&1 || true
systemctl restart rostam.service
ok "rostam.service enabled and started"

# ─── 12. health checks ───────────────────────────────────────────────────────
echo
say "── health ──"
redis-cli ping >/dev/null 2>&1 && ok "redis: PONG" || warn "redis down"
sudo -u postgres psql -d ros_telegram_bot -c 'SELECT 1' >/dev/null 2>&1 && ok "postgres: ok" || warn "postgres db unreachable"
curl -fsS "http://127.0.0.1:6589/health" >/dev/null 2>&1 && ok "separation :6589 up" || warn "separation not ready yet"
[ "$SKIP_ASR" = 0 ] && { curl -fsS "http://127.0.0.1:8765/health" >/dev/null 2>&1 && ok "asr :8765 up" || warn "asr not ready yet (model may still be loading)"; }
[ "$SKIP_BOTAPI" = 0 ] && { curl -fsS "http://127.0.0.1:$BOTAPI_PORT/" >/dev/null 2>&1 && ok "bot-api :$BOTAPI_PORT up" || warn "bot-api not answering yet"; }
sleep 2
systemctl is-active --quiet rostam.service && ok "rostam.service active" || warn "rostam not active — journalctl -u rostam -n 100"

echo
ok "Done. Bot dir: $INSTALL_DIR"
say "Logs: journalctl -u rostam -f"
say "Sidecars: journalctl -u separation -f | -u asr -f | -u telegram-bot-api -f"
[ -f "$INSTALL_DIR/.env" ] && grep -q '^BOT_TOKEN=$' "$INSTALL_DIR/.env" 2>/dev/null && warn "BOT_TOKEN is empty in .env — set it then: systemctl restart rostam"
exit 0
