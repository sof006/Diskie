//! Formatting for sizes, counts, percentages and ages.
//!
//! Sizes use binary units with the labels Windows Explorer uses (1 KB = 1024 bytes), so the
//! numbers match what people see in Explorer's Properties dialog.

pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn percent(p: f64) -> String {
    if p >= 99.95 {
        "100 %".into()
    } else if p > 0.0 && p < 0.05 {
        "<0.1 %".into()
    } else {
        format!("{p:.1} %")
    }
}

/// How long ago `then` (Unix seconds) was, relative to `now`, in a short form: `5 min`, `3 h`, `2 d`, `4 mo`.
pub fn age(then: u32, now: u64) -> String {
    if then == 0 {
        return String::new();
    }
    let s = now.saturating_sub(then as u64);
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    match s {
        s if s < HOUR => format!("{} min", (s / MIN).max(1)),
        s if s < DAY => format!("{} h", s / HOUR),
        s if s < 14 * DAY => format!("{} d", s / DAY),
        s if s < 60 * DAY => format!("{} wk", s / (7 * DAY)),
        s if s < 365 * DAY => format!("{} mo", s / (30 * DAY)),
        s => format!("{} yr", s / (365 * DAY)),
    }
}

pub fn duration(secs: f64) -> String {
    if secs < 10.0 {
        format!("{secs:.1} s")
    } else if secs < 120.0 {
        format!("{secs:.0} s")
    } else {
        format!("{}:{:02} min", secs as u64 / 60, secs as u64 % 60)
    }
}

/// Shortens `s` to at most `max` characters by cutting the middle: `very_lo…name.mkv`.
pub fn middle_truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    let keep = max - 1;
    let tail = keep / 2;
    let head = keep - tail;
    let mut out: String = s.chars().take(head).collect();
    out.push('…');
    out.extend(s.chars().skip(n - tail));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(250 * 1024 * 1024 * 1024), "250 GB");
    }

    #[test]
    fn counts_and_truncation() {
        assert_eq!(count(1_284_113), "1,284,113");
        assert_eq!(count(12), "12");
        assert_eq!(middle_truncate("very_long_file_name.mkv", 12), "very_l…e.mkv");
        assert_eq!(middle_truncate("short", 12), "short");
    }
}
