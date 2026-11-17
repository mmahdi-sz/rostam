pub fn tehran_datetime(epoch: i64) -> Result<jiff::Zoned, jiff::Error> {
    jiff::Timestamp::from_second(epoch)?.in_tz("Asia/Tehran")
}

pub fn gregorian_to_jalali(gy: i32, gm: i32, gd: i32) -> (i32, i32, i32) {
    let g_d_m = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let gy2 = if gm > 2 { gy + 1 } else { gy };
    let mut days = 355_666 + (365 * gy) + ((gy2 + 3) / 4) - ((gy2 + 99) / 100)
        + ((gy2 + 399) / 400)
        + gd
        + g_d_m[(gm - 1) as usize];
    let mut jy = -1595 + 33 * (days / 12053);
    days %= 12053;
    jy += 4 * (days / 1461);
    days %= 1461;
    if days > 365 {
        jy += (days - 1) / 365;
        days = (days - 1) % 365;
    }
    let (jm, jd) = if days < 186 {
        (1 + days / 31, 1 + days % 31)
    } else {
        (7 + (days - 186) / 30, 1 + (days - 186) % 30)
    };
    (jy, jm, jd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tehran_midnight_and_historical_dst() {
        for (utc, expected) in [
            ("2026-03-20T20:30:00Z", "2026-03-21 00:00 +0330"),
            ("2021-07-01T00:00:00Z", "2021-07-01 04:30 +0430"),
            ("2026-07-01T00:00:00Z", "2026-07-01 03:30 +0330"),
        ] {
            let ts: jiff::Timestamp = utc.parse().unwrap();
            let dt = tehran_datetime(ts.as_second()).unwrap();
            assert_eq!(dt.strftime("%Y-%m-%d %H:%M %z").to_string(), expected);
        }
        let dt = tehran_datetime(1774038600).unwrap();
        assert_eq!(
            gregorian_to_jalali(dt.year() as i32, dt.month() as i32, dt.day() as i32),
            (1405, 1, 1)
        );
        assert!(tehran_datetime(i64::MAX).is_err());
        assert!(tehran_datetime(i64::MIN).is_err());
    }
}
