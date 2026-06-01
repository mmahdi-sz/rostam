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

CREATE TABLE IF NOT EXISTS emoji_packs (
    id            SERIAL      PRIMARY KEY,
    owner_user_id BIGINT      NOT NULL,
    name          TEXT        NOT NULL,
    alias         TEXT,
    is_default    BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_user_id, name)
);

CREATE UNIQUE INDEX IF NOT EXISTS emoji_packs_alias_unique
    ON emoji_packs (owner_user_id, alias)
    WHERE alias IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS emoji_packs_default_unique
    ON emoji_packs (owner_user_id)
    WHERE is_default;

CREATE TABLE IF NOT EXISTS emoji_items (
    id              SERIAL  PRIMARY KEY,
    pack_id         INT     NOT NULL REFERENCES emoji_packs(id) ON DELETE CASCADE,
    owner_user_id   BIGINT  NOT NULL,
    custom_emoji_id TEXT    NOT NULL,
    fallback        TEXT    NOT NULL,
    smart_name      TEXT    NOT NULL,
    alias           TEXT,
    position        INT     NOT NULL,
    UNIQUE (owner_user_id, smart_name)
);

CREATE UNIQUE INDEX IF NOT EXISTS emoji_items_alias_unique
    ON emoji_items (owner_user_id, alias)
    WHERE alias IS NOT NULL;

CREATE INDEX IF NOT EXISTS emoji_items_pack_idx
    ON emoji_items (pack_id, position);
