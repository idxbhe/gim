use chrono::{DateTime, Utc};

pub fn format_size(bytes: i64) -> String {
    const KB: f64 = 1024.0; const MB: f64 = KB * 1024.0; const GB: f64 = MB * 1024.0; const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    if b >= TB { format!("{:.1} TB", b / TB) }
    else if b >= GB { format!("{:.1} GB", b / GB) }
    else if b >= MB { format!("{:.1} MB", b / MB) }
    else if b >= KB { format!("{:.1} KB", b / KB) }
    else { format!("{} B", b as i64) }
}

pub fn format_size_compact(bytes: i64) -> String {
    const KB: f64 = 1024.0; const MB: f64 = KB * 1024.0; const GB: f64 = MB * 1024.0;
    let b = bytes.unsigned_abs() as f64;
    let (value, unit) = if b >= GB { (b / GB, "GB") } else if b >= MB { (b / MB, "MB") } else if b >= KB { (b / KB, "KB") } else { (b, "B") };
    let sign = if bytes < 0 { "-" } else { "+" };
    if unit == "B" { format!("{}{}B", sign, b as i64) } else { format!("{}{:.0}{}", sign, value, unit) }
}

pub fn format_timestamp(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp(ms / 1000, 0).unwrap_or_default().format("%Y-%m-%d %H:%M:%S").to_string()
}
