//! Number / time formatting helpers.

use chrono::{DateTime, Utc};

/// Format a byte count as a human-readable string with one decimal
/// place (e.g. `1.2 KB`, `45 MB`, `2.3 GB`).
///
/// Uses binary units (1024-based) per common file-manager convention.
pub fn format_size(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    if b >= TB {
        format!("{:.1} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", b as i64)
    }
}

/// Compact size format without space (e.g. `+45MB`, `-2GB`), used in
/// `g log --oneline` and `g diff --stat` per spec examples.
pub fn format_size_compact(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes.unsigned_abs() as f64;
    let (value, unit) = if b >= GB {
        (b / GB, "GB")
    } else if b >= MB {
        (b / MB, "MB")
    } else if b >= KB {
        (b / KB, "KB")
    } else {
        (b, "B")
    };
    let sign = if bytes < 0 { "-" } else { "+" };
    if unit == "B" {
        format!("{}{}B", sign, b as i64)
    } else {
        format!("{}{:.0}{}", sign, value, unit)
    }
}

/// Format a Unix-millisecond timestamp as `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn format_timestamp(ms: i64) -> String {
    let secs = ms / 1000;
    let dt = DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_kb_mb_gb() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_size_compact_with_sign() {
        assert_eq!(format_size_compact(45 * 1024 * 1024), "+45MB");
        assert_eq!(format_size_compact(-2 * 1024 * 1024 * 1024), "-2GB");
        assert_eq!(format_size_compact(0), "+0B");
    }
}
