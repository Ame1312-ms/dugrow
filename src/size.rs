//! Human-readable size parsing and formatting.

/// Parse a human size like `1024`, `500KB`, `1MiB`, `2.5GB` into bytes.
///
/// Suffixes `KB/MB/GB/TB` are powers of 1000, `KiB/MiB/GiB/TiB` powers of
/// 1024. A bare number (or a `B` suffix) is bytes.
pub fn parse_human_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".to_string());
    }
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, suffix) = s.split_at(split);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("invalid size: {s:?}"))?;
    if value < 0.0 {
        return Err(format!("size must not be negative: {s:?}"));
    }
    let mult: f64 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1_000.0,
        "kib" => 1_024.0,
        "m" | "mb" => 1_000_000.0,
        "mib" => 1_048_576.0,
        "g" | "gb" => 1_000_000_000.0,
        "gib" => 1_073_741_824.0,
        "t" | "tb" => 1_000_000_000_000.0,
        "tib" => 1_099_511_627_776.0,
        other => return Err(format!("unknown size suffix: {other:?}")),
    };
    Ok((value * mult).round() as u64)
}

/// Format a byte count as e.g. `512 B`, `1.5 KiB`, `3.2 GiB`.
pub fn fmt_size(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// Format a signed byte delta as e.g. `+5.0 MiB` / `-2.0 KiB`.
pub fn fmt_delta(d: i64) -> String {
    if d < 0 {
        format!("-{}", fmt_size(d.unsigned_abs()))
    } else {
        format!("+{}", fmt_size(d as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_and_suffixed() {
        assert_eq!(parse_human_size("0").unwrap(), 0);
        assert_eq!(parse_human_size("1024").unwrap(), 1024);
        assert_eq!(parse_human_size("1KB").unwrap(), 1_000);
        assert_eq!(parse_human_size("1KiB").unwrap(), 1_024);
        assert_eq!(parse_human_size("1MiB").unwrap(), 1_048_576);
        assert_eq!(parse_human_size("1mb").unwrap(), 1_000_000);
        assert_eq!(parse_human_size("2.5GiB").unwrap(), 2_684_354_560);
        assert_eq!(parse_human_size("10 MB").unwrap(), 10_000_000);
        assert_eq!(parse_human_size("3B").unwrap(), 3);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_human_size("").is_err());
        assert!(parse_human_size("abc").is_err());
        assert!(parse_human_size("1XB").is_err());
        assert!(parse_human_size("-5").is_err());
    }

    #[test]
    fn format_sizes() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1023), "1023 B");
        assert_eq!(fmt_size(1024), "1.0 KiB");
        assert_eq!(fmt_size(1536), "1.5 KiB");
        assert_eq!(fmt_size(1_048_576), "1.0 MiB");
        assert_eq!(fmt_size(7_516_192_768), "7.0 GiB");
    }

    #[test]
    fn format_deltas() {
        assert_eq!(fmt_delta(0), "+0 B");
        assert_eq!(fmt_delta(5_242_880), "+5.0 MiB");
        assert_eq!(fmt_delta(-2048), "-2.0 KiB");
    }
}
