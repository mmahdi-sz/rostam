#!/bin/bash
# Restore PostgreSQL database from compressed backup file
set -euo pipefail

BACKUP_FILE="${1:-}"
DB_NAME="${2:-ros_telegram_bot}"

if [[ -z "$BACKUP_FILE" || ! -f "$BACKUP_FILE" ]]; then
    echo "Usage: $0 <backup_file.sql.gz> [database_name]"
    exit 1
fi

echo "⚠️  WARNING: This will drop and recreate database '$DB_NAME' from '$BACKUP_FILE'."
read -p "Type 'yes' to proceed: " CONFIRM
if [[ "$CONFIRM" != "yes" ]]; then
    echo "Aborted."
    exit 0
fi

echo "[restore] dropping database '$DB_NAME' if exists..."
dropdb --if-exists "$DB_NAME"

echo "[restore] creating database '$DB_NAME'..."
createdb "$DB_NAME"

echo "[restore] restoring data..."
gunzip -c "$BACKUP_FILE" | psql "$DB_NAME"

echo "[restore] restore completed successfully."
