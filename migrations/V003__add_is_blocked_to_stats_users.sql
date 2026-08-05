-- V002: Add is_blocked tracking to stats_users

ALTER TABLE stats_users ADD COLUMN IF NOT EXISTS is_blocked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE stats_users ADD COLUMN IF NOT EXISTS blocked_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS stats_users_is_blocked_idx ON stats_users (is_blocked);
