CREATE TABLE IF NOT EXISTS cookie_pool_cookies (
    cookie_id TEXT PRIMARY KEY,
    profile_name TEXT NOT NULL,
    profile_dir TEXT NOT NULL,
    cookies_file TEXT NOT NULL,
    updated_at_epoch BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS cookie_pool_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE,
    last_used_cookie TEXT,
    updated_at_epoch BIGINT NOT NULL,
    CONSTRAINT cookie_pool_state_single_row CHECK (id)
);

CREATE TABLE IF NOT EXISTS cookie_pool_cooldowns (
    cookie_id TEXT PRIMARY KEY,
    expire_at_epoch BIGINT NOT NULL
);
