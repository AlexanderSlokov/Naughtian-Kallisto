/// Broken-down UTC time. Deliberately plain: the two callers want different
/// renderings of the same arithmetic, and neither wants a date library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i64,
    pub month: i64,
    pub day: u32,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
}

/// Epoch seconds to broken-down UTC, by Howard Hinnant's `civil_from_days`.
///
/// Shared by the Vault timestamps in responses and by the SigV4 timestamps in
/// `resolver::sigv4`, which needs the same numbers in a different shape.
#[inline]
pub fn civil_from_epoch_secs(total_secs: u64) -> CivilTime {
    let days = total_secs / 86400;
    let rem_secs = total_secs % 86400;

    let z = (days as i64) + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = (mp as i64) + (if mp < 10 { 3 } else { -9 });

    CivilTime {
        year: y + (if month <= 2 { 1 } else { 0 }),
        month,
        day: d,
        hour: rem_secs / 3600,
        minute: (rem_secs % 3600) / 60,
        second: rem_secs % 60,
    }
}

/// Convert epoch milliseconds to RFC 3339 / ISO 8601 string (UTC).
/// Pure arithmetic — no chrono dependency, zero intermediate allocations.
#[inline]
/// The same format, from the clock type the rest of the program carries.
pub fn rfc3339_from_system_time(t: std::time::SystemTime) -> String {
    let ms = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    epoch_ms_to_rfc3339(ms)
}

pub fn epoch_ms_to_rfc3339(ms: u64) -> String {
    if ms == 0 {
        return "1970-01-01T00:00:00Z".to_string();
    }

    let t = civil_from_epoch_secs(ms / 1000);
    let (year, month, d, h, m, s) = (t.year, t.month, t.day, t.hour, t.minute, t.second);
    let fraction_ms = ms % 1000;

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
}
