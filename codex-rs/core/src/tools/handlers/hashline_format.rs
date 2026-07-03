use super::hashline_hash::line_hash;

pub(super) fn format_hashline_excerpt(
    contents: &str,
    start_line: usize,
    end_line: usize,
) -> String {
    if start_line > end_line {
        return String::new();
    }
    split_lines_preserve(contents)
        .into_iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            (line_number >= start_line && line_number <= end_line)
                .then(|| format!("{line_number}:{}|{line}", line_hash(line)))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn split_lines_preserve(contents: &str) -> Vec<&str> {
    let trimmed = contents.strip_suffix('\n').unwrap_or(contents);
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('\n').collect()
    }
}

pub(super) fn count_lines(contents: &str) -> usize {
    split_lines_preserve(contents).len()
}
