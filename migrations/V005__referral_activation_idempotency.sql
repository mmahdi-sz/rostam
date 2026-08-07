-- دو کلیک همزمان روی دکمه‌ی فعال‌سازی، هر دو موجودی یکسانی می‌خوانند و ردیف
-- یکسانی می‌سازند (expires_at از now_epoch با دقت ثانیه محاسبه می‌شود، پس در
-- رقابت واقعی هر دو یک مقدار می‌گیرند). این ایندکس ردیف دوم را غیرممکن می‌کند.
-- فعال‌سازی قانونی بعدی expires_at متفاوتی دارد، پس مسدود نمی‌شود.
CREATE UNIQUE INDEX IF NOT EXISTS referral_activations_idempotency_idx
    ON referral_activations (user_id, rank, expires_at);
