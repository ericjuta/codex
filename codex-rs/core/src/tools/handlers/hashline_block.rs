use std::path::Path;

const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ",
];

pub(super) fn find_block_span(path: &str, lines: &[&str], anchor_line: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (1, 1);
    }

    if let Some(span) = find_markdown_section_span(path, lines, anchor_line) {
        return span;
    }
    if let Some(span) = find_python_header_block_span(path, lines, anchor_line) {
        return span;
    }
    if let Some(span) = find_ruby_block_span(path, lines, anchor_line) {
        return span;
    }
    if let Some(span) = find_brace_block_span(path, lines, anchor_line) {
        return span;
    }
    find_indent_block_span(lines, anchor_line)
}

pub(super) fn language_for_path(path: &str) -> &'static str {
    match extension(path) {
        Some("rs") => "Rust",
        Some("py") => "Python",
        Some("js") => "JavaScript",
        Some("ts") => "TypeScript",
        Some("tsx") => "TSX",
        Some("jsx") => "JSX",
        Some("go") => "Go",
        Some("rb") => "Ruby",
        Some("verse") => "Verse",
        Some("java") => "Java",
        Some("c") => "C",
        Some("cc" | "cpp" | "hpp") => "C++",
        Some("h") => "C/C++ Header",
        Some("cs") => "C#",
        Some("kt" | "kts") => "Kotlin",
        Some("swift") => "Swift",
        Some("scala") => "Scala",
        Some("dart") => "Dart",
        Some("zig") => "Zig",
        Some("m") => "Objective-C",
        Some("mm") => "Objective-C++",
        Some("md" | "mdx") => "Markdown",
        Some("css") => "CSS",
        Some("scss") => "SCSS",
        Some(_) | None => "Unknown",
    }
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

fn find_python_header_block_span(
    path: &str,
    lines: &[&str],
    anchor_line: usize,
) -> Option<(usize, usize)> {
    if !is_python_indent_language(path) {
        return None;
    }
    let anchor_index = anchor_line.checked_sub(1)?;
    let anchor_indent = indent_width(lines[anchor_index]);
    if anchor_indent != 0 {
        return None;
    }

    let end = lines[anchor_index + 1..]
        .iter()
        .position(|line| indent_width(line) <= anchor_indent)
        .map_or(lines.len(), |offset| anchor_index + 1 + offset);

    Some((anchor_index + 1, end))
}

fn find_ruby_block_span(path: &str, lines: &[&str], anchor_line: usize) -> Option<(usize, usize)> {
    if !is_ruby(path) {
        return None;
    }
    let anchor_index = anchor_line.checked_sub(1)?;
    let start = find_ruby_block_start(lines, anchor_index)?;
    let end = find_ruby_block_end(lines, start)?;
    Some((start + 1, end + 1))
}

fn find_ruby_block_start(lines: &[&str], anchor_index: usize) -> Option<usize> {
    let mut depth = 0isize;
    for index in (0..=anchor_index).rev() {
        let trimmed = lines[index].trim();
        depth += ruby_closer_count(trimmed) as isize;
        let open_count = ruby_opener_count(trimmed);
        depth -= open_count as isize;
        if open_count > 0 && depth <= 0 {
            return Some(index);
        }
    }
    None
}

fn find_ruby_block_end(lines: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0isize;
    for (index, line) in lines.iter().enumerate().skip(start) {
        let trimmed = line.trim();
        depth += ruby_opener_count(trimmed) as isize;
        depth -= ruby_closer_count(trimmed) as isize;
        if index > start && depth <= 0 && ruby_closer_count(trimmed) > 0 {
            return Some(index);
        }
        if index == start && depth <= 0 {
            return Some(index);
        }
    }
    None
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

fn is_python_indent_language(path: &str) -> bool {
    extension(path).is_some_and(|extension| matches!(extension, "py" | "verse"))
}

fn is_ruby(path: &str) -> bool {
    extension(path).is_some_and(|extension| extension == "rb")
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

fn ruby_opener_count(trimmed: &str) -> usize {
    usize::from(
        RUBY_OPENERS
            .iter()
            .any(|opener| trimmed.starts_with(*opener)),
    )
}

fn ruby_closer_count(trimmed: &str) -> usize {
    usize::from(trimmed == "end")
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| ch.is_whitespace())
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}
