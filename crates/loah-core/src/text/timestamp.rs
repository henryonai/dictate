//! "mm:ss" / "h:mm:ss" marks, as in transcripts ("[12:34] ...") and the app's timers.

/// `65.0` -> `"01:05"`, `3725.9` -> `"1:02:05"`. Fractions are dropped, not rounded.
pub fn format(seconds: f64) -> String {
    let s = seconds as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// `"12:34"`, `"1:02:03"` or plain seconds (`"754"`), optionally wrapped in
/// `[` `]` or spaces; `None` when it is none of these.
pub fn parse(text: &str) -> Option<f64> {
    let t = text.trim_matches(|c| matches!(c, '[' | ']' | ' '));
    if let Ok(seconds) = t.parse::<f64>() {
        return Some(seconds);
    }
    let parts = t
        .split(':')
        .map(|part| part.parse::<i64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let total = parts.iter().try_fold(0_i64, |acc, &part| {
        acc.checked_mul(60).and_then(|a| a.checked_add(part))
    })?;
    Some(total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format(65.0), "01:05");
        assert_eq!(format(3725.9), "1:02:05");
        assert_eq!(format(0.0), "00:00");
    }

    #[test]
    fn format_switches_to_hours_at_exactly_one_hour() {
        assert_eq!(format(3599.0), "59:59");
        assert_eq!(format(3600.0), "1:00:00");
        assert_eq!(format(36000.0), "10:00:00");
    }

    #[test]
    fn parses() {
        assert_eq!(parse("[12:34]"), Some(754.0));
        assert_eq!(parse("1:02:03"), Some(3723.0));
        assert_eq!(parse("754"), Some(754.0));
        assert_eq!(parse("12:x"), None);
        assert_eq!(parse("1:2:3:4"), None);
    }

    #[test]
    fn parse_trims_brackets_and_spaces() {
        assert_eq!(parse(" [ 12:34 ] "), Some(754.0));
        assert_eq!(parse("[754]"), Some(754.0));
    }

    #[test]
    fn parse_accepts_fractional_seconds() {
        assert_eq!(parse("12.5"), Some(12.5));
    }

    #[test]
    fn parse_rejects_empty_and_dangling_colon() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("[]"), None);
        assert_eq!(parse("12:"), None);
        assert_eq!(parse(":34"), None);
    }

    #[test]
    fn parse_overflow_is_none_not_a_panic() {
        assert_eq!(parse("9223372036854775807:59:59"), None);
    }
}
