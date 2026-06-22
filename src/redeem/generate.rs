use rand::Rng;

use crate::rank::types::Rank;

/// الفبای بدون حروف/ارقام مبهم (بدون 0/O/1/I)
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;

/// کد تصادفی ۸ کاراکتری. فقط حروف بزرگ + ارقام → با payload استارت تلگرام سازگار است.
pub fn random_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CODE_LEN)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// مخفف مقام → Rank
fn rank_from_abbrev(s: &str) -> Option<Rank> {
    match s.to_lowercase().as_str() {
        "da" => Some(Rank::Dalavar),
        "se" => Some(Rank::Sepahbod),
        "es" => Some(Rank::Esfandyar),
        "so" => Some(Rank::Sohrab),
        "ro" => Some(Rank::Rostam),
        other => Rank::from_str(other),
    }
}

/// پارس فلگ‌های ساخت کد: `30d es 1u` (به هر ترتیب).
/// `<n>d` = روز، `<n>u` = تعداد مصرف (پیش‌فرض ۱)، بقیه = مخفف/نام مقام.
/// خروجی: (مقام، روز، تعداد مصرف) یا پیام خطای فارسی.
pub fn parse_gen_args(s: &str) -> Result<(Rank, i32, i32), String> {
    let mut rank: Option<Rank> = None;
    let mut days: Option<i32> = None;
    let mut uses: i32 = 1;

    for tok in s.split_whitespace() {
        let lower = tok.to_lowercase();
        if let Some(num) = lower.strip_suffix('d') {
            if let Ok(n) = num.parse::<i32>() {
                if n > 0 {
                    days = Some(n);
                    continue;
                }
            }
            return Err(format!("مدت نامعتبر: «{tok}». مثال: 30d"));
        }
        if let Some(num) = lower.strip_suffix('u') {
            if let Ok(n) = num.parse::<i32>() {
                if n > 0 {
                    uses = n;
                    continue;
                }
            }
            return Err(format!("تعداد مصرف نامعتبر: «{tok}». مثال: 5u"));
        }
        match rank_from_abbrev(tok) {
            Some(r) => rank = Some(r),
            None => return Err(format!("مقام نامعتبر: «{tok}». مقام‌ها: da/se/es/so/ro")),
        }
    }

    let Some(rank) = rank else {
        return Err("مقام مشخص نشده. مثال: 30d es 1u".to_string());
    };
    let Some(days) = days else {
        return Err("مدت مشخص نشده. مثال: 30d es 1u".to_string());
    };

    Ok((rank, days, uses))
}
