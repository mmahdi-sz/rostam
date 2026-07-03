#!/usr/bin/env bash
set -Eeuo pipefail

REPO_DIR="/home/mmahdi-sz/Desktop/codes/rostam"
SERVER="mahdi"
SERVER_DEPLOY_DIR="/mnt/data/mahdidev/ros"

cd "$REPO_DIR"

branch="$(git branch --show-current)"
if [[ "$branch" != "dev" ]]; then
    echo "ERROR: deploy must run from branch 'dev'. Current branch: $branch"
    exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
    echo "ERROR: uncommitted changes exist. Commit or stash them first:"
    git status --short
    exit 1
fi

echo "==> pushing dev to GitHub..."
git push origin dev

echo "==> running deploy on $SERVER..."
ssh "$SERVER" "cd '$SERVER_DEPLOY_DIR' && ./deploy.sh"
