#!/bin/bash
# Backup PostgreSQL database with retention policy
set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/mnt/data/backups/ros-telegram-bot}"
DB_NAME="${1:-ros_telegram_bot}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

mkdir -p "$BACKUP_DIR"

echo "[backup] dumping database '$DB_NAME' to '$BACKUP_DIR'..."
pg_dump "$DB_NAME" | gzip > "$BACKUP_DIR/${DB_NAME}_${TIMESTAMP}.sql.gz"

echo "[backup] removing backups older than $RETENTION_DAYS days..."
find "$BACKUP_DIR" -name "${DB_NAME}_*.sql.gz" -mtime +$RETENTION_DAYS -delete

REMAINING=$(ls -1 "$BACKUP_DIR"/${DB_NAME}_*.sql.gz 2>/dev/null | wc -l || echo 0)
echo "[backup] completed successfully: ${DB_NAME}_${TIMESTAMP}.sql.gz (total stored: $REMAINING)"
