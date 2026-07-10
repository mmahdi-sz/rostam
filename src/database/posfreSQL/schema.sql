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

-- ── stats ────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS stats_users (
    user_id         BIGINT      PRIMARY KEY,
    first_seen      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    first_upload_at BIGINT      -- Unix epoch اولین آپلود موفق
);

ALTER TABLE stats_users ADD COLUMN IF NOT EXISTS first_upload_at BIGINT;
ALTER TABLE stats_users ADD COLUMN IF NOT EXISTS language TEXT;

CREATE TABLE IF NOT EXISTS stats_downloads (
    id              BIGSERIAL   PRIMARY KEY,
    user_id         BIGINT      NOT NULL,
    bytes_downloaded BIGINT     NOT NULL DEFAULT 0,
    bytes_uploaded   BIGINT     NOT NULL DEFAULT 0,
    upload_ok        BOOLEAN    NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS stats_downloads_created_idx
    ON stats_downloads (created_at);

CREATE INDEX IF NOT EXISTS stats_downloads_user_idx
    ON stats_downloads (user_id);

-- رویداد عمومی هر فیچر (stt/denoise/upscale/separation/gwm/asr/...).
-- amount = ثانیه‌ی صدا یا تعداد، بسته به فیچر.
CREATE TABLE IF NOT EXISTS stats_events (
    id         BIGSERIAL   PRIMARY KEY,
    user_id    BIGINT      NOT NULL DEFAULT 0,
    feature    TEXT        NOT NULL,
    action     TEXT        NOT NULL DEFAULT '',
    status     TEXT        NOT NULL DEFAULT 'ok',
    amount     BIGINT      NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS stats_events_created_idx
    ON stats_events (created_at);

CREATE INDEX IF NOT EXISTS stats_events_feature_idx
    ON stats_events (feature, created_at);

-- خطاهای ثبت‌شده‌ی فیچرها برای دکمه «خطاهای ۱ روز گذشته».
CREATE TABLE IF NOT EXISTS stats_errors (
    id         BIGSERIAL   PRIMARY KEY,
    feature    TEXT        NOT NULL,
    message    TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS stats_errors_created_idx
    ON stats_errors (created_at);

-- ── ranks ────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS user_ranks (
    user_id      BIGINT  PRIMARY KEY,
    rank         TEXT    NOT NULL DEFAULT 'dalavar',
    expires_at   BIGINT,          -- Unix timestamp, NULL = نامحدود
    activated_at BIGINT  NOT NULL DEFAULT EXTRACT(EPOCH FROM NOW())::BIGINT
);

-- ── quotas ───────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS user_quotas (
    user_id      BIGINT NOT NULL,
    quota_type   TEXT   NOT NULL,
    used         BIGINT NOT NULL DEFAULT 0,
    window_start BIGINT NOT NULL,
    PRIMARY KEY (user_id, quota_type)
);

-- ── redeem codes ─────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS redeem_codes (
    code          TEXT    PRIMARY KEY,
    rank          TEXT    NOT NULL,
    duration_days INT     NOT NULL,
    max_uses      INT     NOT NULL DEFAULT 1,
    used_count    INT     NOT NULL DEFAULT 0,
    created_by    BIGINT,
    created_at    BIGINT  NOT NULL DEFAULT EXTRACT(EPOCH FROM NOW())::BIGINT
);

CREATE TABLE IF NOT EXISTS redeem_redemptions (
    code        TEXT   NOT NULL,
    user_id     BIGINT NOT NULL,
    redeemed_at BIGINT NOT NULL,
    PRIMARY KEY (code, user_id)   -- یک کاربر هر کد را فقط یک‌بار
);

-- عمر کشویی ۷ روزه: موقع ساخت now+7d، هر مصرف ریست می‌شود، بعد از انقضا پاک می‌شود.
ALTER TABLE redeem_codes ADD COLUMN IF NOT EXISTS expires_at BIGINT;

-- ── زیرمجموعه‌گیری (referral) ───────────────────────────────────────────────
-- هر کاربر فقط یک‌بار می‌تواند زیرمجموعه‌ی کسی محسوب شود (referred_id = PK).
-- referrals = دعوت‌های تأییدشده (شمرده می‌شوند، امتیاز می‌دهند).

CREATE TABLE IF NOT EXISTS referrals (
    referred_id BIGINT PRIMARY KEY,
    referrer_id BIGINT NOT NULL,
    created_at  BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS referrals_referrer_idx
    ON referrals (referrer_id);

-- referral_pending = دعوت‌های در انتظار تأیید: کاربر با لینک استارت کرده ولی هنوز
-- ۲ روز از عضویتش در قفل اجباری نگذشته. sweep دوره‌ای (referral::sweep_confirm)
-- بعد از ۲ روز عضویت رو دوباره چک می‌کند: عضو بود → به referrals منتقل می‌شود؛
-- عضو نبود → حذف می‌شود (دعوت باطل).
CREATE TABLE IF NOT EXISTS referral_pending (
    referred_id BIGINT PRIMARY KEY,
    referrer_id BIGINT NOT NULL,
    started_at  BIGINT NOT NULL
);

-- تاریخچه‌ی فعال‌سازی رتبه با امتیاز دعوت. points_spent برای محاسبه‌ی موجودی
-- قابل‌خرج جمع زده می‌شود (موجودی = COUNT(referrals) - SUM(points_spent)).
CREATE TABLE IF NOT EXISTS referral_activations (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL,
    rank         TEXT   NOT NULL,
    points_spent INT    NOT NULL,
    activated_at BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS referral_activations_user_idx
    ON referral_activations (user_id);
