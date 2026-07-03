use std::path::Path;

pub(super) fn find_block_span(path: &str, lines: &[&str], anchor_line: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (1, 1);
    }

    if let Some(span) = find_markdown_section_span(path, lines, anchor_line) {
        return span;
    }
    if let Some(span) = find_brace_block_span(path, lines, anchor_line) {
        return span;
    }
    find_indent_block_span(lines, anchor_line)
}

fn find_brace_block_span(path: &str, lines: &[&str], anchor_line: usize) -> Option<(usize, usize)> {
    if !is_brace_language(path) {
        return None;
    }
    let anchor_index = anchor_line.checked_sub(1)?;
    let mut stack = Vec::new();
    let mut best = None;

    for (line_index, line) in lines.iter().enumerate() {
        let mut string_delimiter = None;
        let mut escaped = false;
        for ch in line.chars() {
            if let Some(delimiter) = string_delimiter {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == delimiter {
                    string_delimiter = None;
                }
                continue;
            }

            match ch {
                '"' | '\'' | '`' => string_delimiter = Some(ch),
                '{' => stack.push(line_index),
                '}' => {
                    let Some(open_line) = stack.pop() else {
                        continue;
                    };
                    if open_line <= anchor_index && anchor_index <= line_index {
                        let candidate = (open_line + 1, line_index + 1);
                        best = match best {
                            Some(current) if span_len(current) <= span_len(candidate) => {
                                Some(current)
                            }
                            _ => Some(candidate),
                        };
                    }
                }
                _ => {}
            }
        }
    }

    best
}

fn find_markdown_section_span(
    path: &str,
    lines: &[&str],
    anchor_line: usize,
) -> Option<(usize, usize)> {
    if !is_markdown(path) {
        return None;
    }
    let anchor_index = anchor_line.checked_sub(1)?;
    let start_index = lines[..=anchor_index]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, line)| markdown_heading_level(line).map(|level| (index, level)))?;
    let (start, start_level) = start_index;
    let end = lines[start + 1..]
        .iter()
        .position(|line| markdown_heading_level(line).is_some_and(|level| level <= start_level))
        .map_or(lines.len(), |offset| start + 1 + offset);

    Some((start + 1, end))
}

fn find_indent_block_span(lines: &[&str], anchor_line: usize) -> (usize, usize) {
    let anchor_index = anchor_line - 1;
    let anchor_indent = indent_width(lines[anchor_index]);
    let mut start = anchor_index;
    while start > 0 {
        let previous = lines[start - 1];
        if !previous.trim().is_empty() && indent_width(previous) < anchor_indent {
            break;
        }
        start -= 1;
    }

    let mut end = anchor_index;
    while end + 1 < lines.len() {
        let next = lines[end + 1];
        if !next.trim().is_empty() && indent_width(next) < anchor_indent {
            break;
        }
        end += 1;
    }

    (start + 1, end + 1)
}

fn span_len(span: (usize, usize)) -> usize {
    span.1.saturating_sub(span.0)
}

fn is_brace_language(path: &str) -> bool {
    extension(path).is_some_and(|extension| {
        matches!(
            extension,
            "c" | "cc"
                | "cpp"
                | "cs"
                | "css"
                | "go"
                | "h"
                | "hpp"
                | "java"
                | "js"
                | "jsx"
                | "kt"
                | "rs"
                | "scss"
                | "swift"
                | "ts"
                | "tsx"
        )
    })
}

fn is_markdown(path: &str) -> bool {
    extension(path).is_some_and(|extension| matches!(extension, "md" | "mdx"))
}

fn extension(path: &str) -> Option<&str> {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
}

fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && trimmed.as_bytes().get(level) == Some(&b' ') {
        Some(level)
    } else {
        None
    }
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| ch.is_whitespace())
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}
