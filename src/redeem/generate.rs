use rand::Rng;

use crate::i18n::tf;
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
            return Err(tf("redeem.gen_invalid_days", &[("tok", tok)]));
        }
        if let Some(num) = lower.strip_suffix('u') {
            if let Ok(n) = num.parse::<i32>() {
                if n > 0 {
                    uses = n;
                    continue;
                }
            }
            return Err(tf("redeem.gen_invalid_uses", &[("tok", tok)]));
        }
        match rank_from_abbrev(tok) {
            Some(r) => rank = Some(r),
            None => return Err(tf("redeem.gen_invalid_rank", &[("tok", tok)])),
        }
    }

    let Some(rank) = rank else {
        return Err(crate::i18n::t("redeem.gen_missing_rank"));
    };
    let Some(days) = days else {
        return Err(crate::i18n::t("redeem.gen_missing_days"));
    };

    Ok((rank, days, uses))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_code_length_and_alphabet() {
        let code = random_code();
        assert_eq!(code.len(), 8);
        for c in code.chars() {
            assert!(ALPHABET.contains(&(c as u8)));
        }
    }

    #[test]
    fn test_parse_gen_args_valid() {
        let (rank, days, uses) = parse_gen_args("30d es 5u").unwrap();
        assert_eq!(rank, Rank::Esfandyar);
        assert_eq!(days, 30);
        assert_eq!(uses, 5);
    }

    #[test]
    fn test_parse_gen_args_invalid_rank() {
        let res = parse_gen_args("30d invalid_rank 1u");
        assert!(res.is_err());
    }
}
