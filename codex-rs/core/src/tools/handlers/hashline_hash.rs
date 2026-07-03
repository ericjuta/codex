use xxhash_rust::xxh3::xxh3_64;
use xxhash_rust::xxh32::xxh32;

pub(super) fn hash_hex(input: &str, width: usize) -> String {
    let normalized = normalize_file_text(input);
    let hash = xxh3_64(normalized.as_bytes());
    let shift = 64usize.saturating_sub(width.saturating_mul(4));
    let value = if shift == 0 { hash } else { hash >> shift };
    format!("{value:0width$x}")
}

pub(super) fn line_hash(input: &str) -> String {
    format!(
        "{:02x}",
        (xxh32(input.trim_end().as_bytes(), 0) & 0xff) as u8
    )
}

fn normalize_file_text(input: &str) -> String {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            output.push('\n');
        } else {
            output.push(ch);
        }
    }
    output
}
