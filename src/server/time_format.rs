/// Convert epoch milliseconds to RFC 3339 / ISO 8601 string (UTC).
/// Pure arithmetic — no chrono dependency, zero intermediate allocations.
#[inline]
pub fn epoch_ms_to_rfc3339(ms: u64) -> String {
    if ms == 0 {
        return "1970-01-01T00:00:00Z".to_string();
    }

    let total_secs = ms / 1000;

    // Civil time arithmetic
    let days = total_secs / 86400;
    let rem_secs = total_secs % 86400;
    let h = rem_secs / 3600;
    let m = (rem_secs % 3600) / 60;
    let s = rem_secs % 60;
    let fraction_ms = ms % 1000;

    let z = (days as i64) + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = (mp as i64) + (if mp < 10 { 3 } else { -9 });
    let year = y + (if month <= 2 { 1 } else { 0 });

    let mut buf = String::with_capacity(30);

    // Using simple format! since we don't have itoa in deps, but pre-allocated buf
    // minimizes impact.
    use std::fmt::Write;
    if fraction_ms > 0 {
        let _ = write!(
            &mut buf,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year, month, d, h, m, s, fraction_ms
        );
    } else {
        let _ = write!(
            &mut buf,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, d, h, m, s
        );
    }

    buf
}

/// Parse Go-style duration "3h25m19s" → milliseconds.
/// Single-pass byte scanner — no regex, no allocations.
#[inline]
pub fn parse_vault_duration(s: &str) -> Result<u64, &'static str> {
    if s.is_empty() {
        return Ok(0);
    }

    let mut ms: u64 = 0;
    let mut current_num: u64 = 0;
    let mut has_num = false;

    for b in s.bytes() {
        match b {
            b'0'..=b'9' => {
                current_num = current_num * 10 + (b - b'0') as u64;
                has_num = true;
            }
            b'h' => {
                ms += current_num * 3_600_000;
                current_num = 0;
                has_num = false;
            }
            b'm' => {
                ms += current_num * 60_000;
                current_num = 0;
                has_num = false;
            }
            b's' => {
                ms += current_num * 1_000;
                current_num = 0;
                has_num = false;
            }
            _ => return Err("invalid duration character"),
        }
    }

    if has_num {
        return Err("missing unit in duration");
    }

    Ok(ms)
}

/// Format milliseconds → Go-style duration "3h25m19s"
#[inline]
pub fn ms_to_vault_duration(ms: u64) -> String {
    if ms == 0 {
        return "0s".to_string();
    }

    let secs = ms / 1000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    let mut buf = String::with_capacity(20);
    use std::fmt::Write;

    if h > 0 {
        let _ = write!(&mut buf, "{}h", h);
    }
    if m > 0 {
        let _ = write!(&mut buf, "{}m", m);
    }
    if s > 0 || (h == 0 && m == 0) {
        let _ = write!(&mut buf, "{}s", s);
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch_ms_to_rfc3339() {
        assert_eq!(epoch_ms_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(epoch_ms_to_rfc3339(1672531200000), "2023-01-01T00:00:00Z");
        assert_eq!(
            epoch_ms_to_rfc3339(1672531200123),
            "2023-01-01T00:00:00.123Z"
        );
    }

    #[test]
    fn test_parse_vault_duration() {
        assert_eq!(parse_vault_duration("3h").unwrap(), 3 * 3600 * 1000);
        assert_eq!(
            parse_vault_duration("3h25m19s").unwrap(),
            (3 * 3600 + 25 * 60 + 19) * 1000
        );
        assert!(parse_vault_duration("3").is_err());
        assert!(parse_vault_duration("3x").is_err());
    }

    #[test]
    fn test_ms_to_vault_duration() {
        assert_eq!(ms_to_vault_duration(0), "0s");
        assert_eq!(ms_to_vault_duration(3000), "3s");
        assert_eq!(ms_to_vault_duration(60000), "1m");
        assert_eq!(
            ms_to_vault_duration((3 * 3600 + 25 * 60 + 19) * 1000),
            "3h25m19s"
        );
    }
}
