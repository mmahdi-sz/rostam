#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR="/opt/rostam"
REPO="${ROSTAM_REPO:-https://github.com/mmahdi-sz/rostam.git}"
BRANCH="${ROSTAM_BRANCH:-master}"
DENO_DIR="/opt/deno"
BOTAPI_PORT=8081

FRESH=0
SKIP_BOTAPI=0
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
(separation :6589, surge :1700, local Telegram Bot API :8081) as
systemd services.

Usage:
  bash <(curl -Ls https://raw.githubusercontent.com/mmahdi-sz/rostam/master/install.sh)
  sudo bash install.sh [options]

Options:
  --dir <path>      install location (default /opt/rostam)
  --branch <name>   git branch to clone (default master)
  --skip-bot-api    do not build/install the local Telegram Bot API server
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
    $([ "$SKIP_FIREFOX" = 1 ] && echo --skip-firefox) \
    $([ "$FRESH" = 1 ] && echo --fresh)
fi

. /etc/os-release 2>/dev/null || die "cannot read /etc/os-release"
case "${ID:-}:${ID_LIKE:-}" in
  *arch*)            FAMILY="arch" ;;
  *debian*|*ubuntu*) FAMILY="debian" ;;
  *) die "unsupported distro: ID=${ID:-?} (only Arch & Debian/Ubuntu)" ;;
esac
say "distro=${PRETTY_NAME:-$ID} family=$FAMILY install_dir=$INSTALL_DIR branch=$BRANCH"

pac() {
  local rc=0
  set +o pipefail
  yes '' 2>/dev/null | pacman "$@" || rc=$?
  set -o pipefail
  return "$rc"
}

fetch() {
  if [ -n "${ROSTAM_ASSET_CACHE:-}" ] && [ -f "$ROSTAM_ASSET_CACHE/$(basename "$2")" ]; then
    say "cache hit: $(basename "$2")"
    cp "$ROSTAM_ASSET_CACHE/$(basename "$2")" "$2"
    return 0
  fi
  curl -fL --retry 3 --retry-delay 2 -o "$2" "$1"
}

need_file() {
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

say "installing system packages…"
if [ "$FAMILY" = "debian" ]; then
  export DEBIAN_FRONTEND=noninteractive
  for f in /etc/apt/sources.list.d/*.sources; do
    [ -f "$f" ] || continue
    grep -qE '^Components:.*non-free' "$f" && continue
    sed -i -E 's/^(Components:.*)$/\1 contrib non-free non-free-firmware/' "$f"
  done
  if [ -f /etc/apt/sources.list ] && grep -qE '^deb ' /etc/apt/sources.list \
     && ! grep -qE '^deb .*non-free' /etc/apt/sources.list; then
    sed -i -E '/^deb .*(debian|ubuntu)/ s/$/ contrib non-free non-free-firmware/' /etc/apt/sources.list
  fi
  apt-get update -qq
  PKGS="git curl ca-certificates unzip tar xz-utils build-essential pkg-config \
        ffmpeg ghostscript redis-server postgresql postgresql-client \
        python3 python3-venv python3-pip procps cmake g++ make gperf zlib1g-dev libssl-dev \
        espeak-ng p7zip-full sudo clang libclang-dev"
  [ "$SKIP_FIREFOX" = 0 ] && PKGS="$PKGS firefox-esr"
  apt-get install -y --no-install-recommends $PKGS
  apt-get install -y --no-install-recommends rar 2>/dev/null || warn "rar unavailable (surge archive-split degrades)"
else
  PKGS="git curl ca-certificates unzip tar xz base-devel ffmpeg ghostscript \
        redis postgresql python python-pip procps cmake gperf espeak-ng p7zip sudo clang"
  [ "$SKIP_FIREFOX" = 0 ] && PKGS="$PKGS firefox"
  pac -Sy --noconfirm --needed archlinux-keyring
  pac -Syu --noconfirm --needed $PKGS
fi
ok "system packages ready"

if ! command -v cargo >/dev/null 2>&1; then
  say "installing Rust via rustup…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
# shellcheck disable=SC1090
. "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || die "cargo not on PATH after rustup"
ok "rust $(rustc --version | awk '{print $2}')"

if [ ! -x "$DENO_DIR/bin/deno" ]; then
  say "installing deno → $DENO_DIR…"
  curl -fsSL https://deno.land/install.sh | env DENO_INSTALL="$DENO_DIR" sh -s -- -y >/dev/null 2>&1 || true
fi
[ -x "$DENO_DIR/bin/deno" ] && ok "deno ready ($DENO_DIR/bin/deno)" || warn "deno install failed (YouTube JS challenges degrade)"

if ! command -v yt-dlp >/dev/null 2>&1; then
  say "installing yt-dlp…"
  fetch "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp" /usr/local/bin/yt-dlp
  chmod +x /usr/local/bin/yt-dlp
fi
ok "yt-dlp ready"

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

say "provisioning model assets (this downloads several GB)…"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"; die "failed at line $LINENO (exit $?)"' ERR

if [ ! -f files/runtime/libvosk.so ]; then
  say "downloading libvosk…"
  fetch "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip" "$TMP/vosk.zip"
  mkdir -p files/runtime
  unzip -o "$TMP/vosk.zip" -d "$TMP/vosk" >/dev/null
  cp "$(find "$TMP/vosk" -name libvosk.so | head -1)" files/runtime/libvosk.so
  ok "libvosk.so"
else ok "libvosk.so present — skipping"; fi

mkdir -p files/models/vosk
for m in vosk-model-fa-0.42 vosk-model-small-fa-0.42 vosk-model-en-us-0.22-lgraph vosk-model-small-en-us-0.15; do
  if [ -d "files/models/vosk/$m" ]; then ok "vosk/$m present — skipping"; continue; fi
  say "downloading vosk/$m…"
  fetch "https://alphacephei.com/vosk/models/$m.zip" "$TMP/$m.zip"
  unzip -o "$TMP/$m.zip" -d files/models/vosk >/dev/null
  rm -f "$TMP/$m.zip"
  ok "vosk/$m"
done

need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/unet.onnx"        files/models/moebius/unet.onnx        800000000 "moebius/unet.onnx"
need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/vae_decoder.onnx" files/models/moebius/vae_decoder.onnx 150000000 "moebius/vae_decoder.onnx"
need_file "https://huggingface.co/simonw/Moebius-ONNX/resolve/main/vae_encoder.onnx" files/models/moebius/vae_encoder.onnx 100000000 "moebius/vae_encoder.onnx"

need_file "https://github.com/Rikorose/DeepFilterNet/raw/1e96ef05e1ef75b3702f8c55ca065368deae637d/models/DeepFilterNet3_onnx.tar.gz" \
          files/models/deepfilter/DeepFilterNet3_onnx.tar.gz 5000000 "deepfilter model"
if [ ! -x files/runtime/deep-filter ]; then
  say "downloading deep-filter binary…"
  if fetch "https://github.com/Rikorose/DeepFilterNet/releases/download/v0.5.6/deep-filter-0.5.6-x86_64-unknown-linux-musl" files/runtime/deep-filter; then
    chmod +x files/runtime/deep-filter; ok "deep-filter binary"
  else warn "deep-filter binary unavailable — STT denoise will degrade"; fi
else ok "deep-filter binary present — skipping"; fi

if [ ! -x files/realesrgan/realesrgan-ncnn-vulkan ]; then
  say "downloading Real-ESRGAN…"
  fetch "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.5.0/realesrgan-ncnn-vulkan-20220424-ubuntu.zip" "$TMP/resrgan.zip"
  mkdir -p files/realesrgan
  unzip -o "$TMP/resrgan.zip" -d files/realesrgan >/dev/null
  chmod +x files/realesrgan/realesrgan-ncnn-vulkan
  ok "Real-ESRGAN"
else ok "Real-ESRGAN present — skipping"; fi

PIPER_REPO="https://huggingface.co/kiarashQ/fa-ir-tts-piper-ar-mantatts-v1/resolve/main"
need_file "$PIPER_REPO/fa_IR-mantatts-par.onnx"      models/piper/fa_IR/fa_IR-mantatts-par.onnx      200000000 "piper fa_IR model"
need_file "$PIPER_REPO/fa_IR-mantatts-par.onnx.json" models/piper/fa_IR/fa_IR-mantatts-par.onnx.json 1000       "piper fa_IR config"

need_file "https://huggingface.co/mmahdi-sz/FeyNobg-ONNX/resolve/main/feynobg_int8_dynamic.onnx" \
          files/models/feynobg/feynobg_int8_dynamic.onnx 1000000000 "feynobg model"

need_file "https://huggingface.co/JustFrederik/nllb-200-distilled-600M-ct2-int8/resolve/main/model.bin" \
          files/models/nllb/model.bin 500000000 "nllb model.bin"
need_file "https://huggingface.co/entai2965/nllb-200-distilled-600M-ctranslate2/resolve/main/shared_vocabulary.json" \
          files/models/nllb/shared_vocabulary.json 1000000 "nllb vocabulary"
need_file "https://huggingface.co/entai2965/nllb-200-distilled-600M-ctranslate2/resolve/main/tokenizer.json" \
          files/models/nllb/tokenizer.json 1000000 "nllb tokenizer"
if [ ! -f files/models/nllb/config.json ]; then
  cat > files/models/nllb/config.json <<'JSON'
{
  "add_source_bos": false,
  "add_source_eos": false,
  "bos_token": "<s>",
  "decoder_start_token": "</s>",
  "eos_token": "</s>",
  "layer_norm_epsilon": null,
  "multi_query_attention": false,
  "unk_token": "<unk>"
}
JSON
  ok "nllb config.json written"
else ok "nllb config.json present — skipping"; fi

DDCOLOR_SHA=03d13d21b08cac6d9f6216cb79538b5826c240f910dbc55911dbb70ce280944b
ddcolor_fetch() {
  local id=1R05w65qEWW_H4lcW1NdfcqAEPdWvjqEW uuid
  if [ -n "${ROSTAM_ASSET_CACHE:-}" ] && [ -f "$ROSTAM_ASSET_CACHE/ddcolor_modelscope.onnx" ]; then
    mkdir -p files/models/deoldify
    cp "$ROSTAM_ASSET_CACHE/ddcolor_modelscope.onnx" files/models/deoldify/ || return 1
  else
    say "downloading ddcolor models zip (1.9 GB, Google Drive)…"
    curl -sL -c "$TMP/gck" "https://drive.usercontent.google.com/download?id=$id&export=download" -o "$TMP/gd.html" || return 1
    uuid="$(grep -oE 'name="uuid" value="[^"]*"' "$TMP/gd.html" | cut -d'"' -f4)"
    [ -n "$uuid" ] || return 1
    curl -fL --retry 3 --retry-delay 2 -b "$TMP/gck" \
      "https://drive.usercontent.google.com/download?id=$id&export=download&confirm=t&uuid=$uuid" \
      -o "$TMP/ddc.zip" || return 1
    mkdir -p files/models/deoldify
    unzip -o -q "$TMP/ddc.zip" ddcolor_modelscope.onnx -d files/models/deoldify || return 1
    rm -f "$TMP/ddc.zip"
  fi
  [ "$(sha256sum files/models/deoldify/ddcolor_modelscope.onnx | cut -d' ' -f1)" = "$DDCOLOR_SHA" ] \
    || { rm -f files/models/deoldify/ddcolor_modelscope.onnx; return 1; }
}
if [ -s files/models/deoldify/ddcolor_modelscope.onnx ]; then
  ok "deoldify/ddcolor_modelscope.onnx present"
elif ddcolor_fetch; then
  ok "deoldify/ddcolor_modelscope.onnx downloaded (sha256 verified)"
else
  warn "ddcolor_modelscope.onnx unavailable — /colorize will fail."
  warn "  Grab ddcolorizer_onnx_models.zip from instant-high/DDColor-onnx's README"
  warn "  (Google Drive), extract ddcolor_modelscope.onnx into files/models/deoldify/,"
  warn "  then restart rostam."
fi

rm -rf "$TMP"
trap 'die "failed at line $LINENO (exit $?)"' ERR
ok "assets ready"

say "configuring PostgreSQL…"
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

say "configuring Redis…"
REDIS_UNIT=""
UNIT_FILES="$(systemctl list-unit-files --type=service 2>/dev/null || true)"
for u in redis-server redis valkey; do
  grep -q "^${u}\.service" <<<"$UNIT_FILES" && { REDIS_UNIT="$u"; break; }
done
[ -n "$REDIS_UNIT" ] || die "no redis/valkey unit after install"
systemctl enable --now "$REDIS_UNIT" >/dev/null 2>&1 || systemctl restart "$REDIS_UNIT"
redis-cli ping 2>/dev/null | grep -qi pong && ok "Redis up (unit=$REDIS_UNIT)" || warn "redis PING failed"

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
  set_env SURGE_DOWNLOADS_ROOT "$INSTALL_DIR/downloads/surge"
  [ "$SKIP_BOTAPI" = 1 ] && set_env BOT_API_BASE_URL "" || set_env BOT_API_BASE_URL "http://127.0.0.1:$BOTAPI_PORT"
  chmod 600 .env
  ok ".env written"
else
  ok ".env exists — leaving as-is"
fi

say "building the bot (cargo build --release — this takes a while)…"
cargo build --release
[ -x target/release/rostam-dev ] || die "bot binary missing after build"
ok "bot built"


say "installing separation-service (:6589)…"
bash "$INSTALL_DIR/separation-service/setup.sh" || warn "separation-service install failed (:6589 degrades)"

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

surge_install() {
  local dl="$INSTALL_DIR/downloads/surge"; mkdir -p "$dl"
  if ! command -v surge >/dev/null 2>&1; then
    say "installing surge (SurgeDM/Surge)…"
    local url st; st="$(mktemp -d)"
    url="$(curl -fsSL https://api.github.com/repos/SurgeDM/Surge/releases/latest \
           | grep -oE 'https://[^"]+/Surge_[0-9.]+_linux_amd64\.tar\.gz' | head -1)"
    [ -n "$url" ] || { rm -rf "$st"; return 1; }
    fetch "$url" "$st/surge.tgz"                || { rm -rf "$st"; return 1; }
    tar xzf "$st/surge.tgz" -C "$st"            || { rm -rf "$st"; return 1; }
    install -m 0755 "$st/surge" /usr/local/bin/surge || { rm -rf "$st"; return 1; }
    rm -rf "$st"
  fi
  systemctl disable --now surge.service >/dev/null 2>&1 || true
  rm -f /etc/systemd/system/surge.service
  cat > /etc/systemd/system/rostam-surge.service <<UNIT
[Unit]
Description=Surge download manager daemon
After=network.target

[Service]
Type=simple
User=root
Environment=HOME=/root
ExecStart=/usr/local/bin/surge server start -p 1700 -o $dl
Restart=on-failure

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now rostam-surge.service >/dev/null 2>&1 || systemctl restart rostam-surge.service
}
say "installing surge download manager (:1700)…"
if surge_install; then ok "rostam-surge.service installed (:1700)"
else warn "surge install failed (tools:surge degrades; core bot unaffected)"; fi

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
ExecStart=$INSTALL_DIR/target/release/rostam-dev
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable rostam.service >/dev/null 2>&1 || true
systemctl restart rostam.service
ok "rostam.service enabled and started"

echo
say "── health ──"
redis-cli ping >/dev/null 2>&1 && ok "redis: PONG" || warn "redis down"
sudo -u postgres psql -d ros_telegram_bot -c 'SELECT 1' >/dev/null 2>&1 && ok "postgres: ok" || warn "postgres db unreachable"
curl -fsS "http://127.0.0.1:6589/health" >/dev/null 2>&1 && ok "separation :6589 up" || warn "separation not ready yet"
HOME=/root surge --host 127.0.0.1:1700 ls --json >/dev/null 2>&1 && ok "surge :1700 up" || warn "surge not ready yet"
[ "$SKIP_BOTAPI" = 0 ] && { curl -fsS "http://127.0.0.1:$BOTAPI_PORT/" >/dev/null 2>&1 && ok "bot-api :$BOTAPI_PORT up" || warn "bot-api not answering yet"; }
sleep 2
systemctl is-active --quiet rostam.service && ok "rostam.service active" || warn "rostam not active — journalctl -n 100 -u rostam"

echo
ok "Done. Bot dir: $INSTALL_DIR"
say "Logs: journalctl -u rostam -f"
say "Sidecars: journalctl -u separation -f | -u rostam-surge -f | -u telegram-bot-api -f"
[ -f "$INSTALL_DIR/.env" ] && grep -q '^BOT_TOKEN=$' "$INSTALL_DIR/.env" 2>/dev/null && warn "BOT_TOKEN is empty in .env — set it then: systemctl restart rostam"
exit 0
