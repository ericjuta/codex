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

pub(super) fn find_normalized_block_span(
    path: &str,
    lines: &[&str],
    anchor_line: usize,
) -> (usize, usize) {
    let (start, mut end) = find_block_span(path, lines, anchor_line);
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    (start, end)
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
    let mut in_block_comment = false;

    for (line_index, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut index = 0;
        let mut string_delimiter = None;
        let mut escaped = false;

        while index < bytes.len() {
            let byte = bytes[index];
            if in_block_comment {
                if index + 1 < bytes.len() && byte == b'*' && bytes[index + 1] == b'/' {
                    in_block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if let Some(delimiter) = string_delimiter {
                if escaped {
                    escaped = false;
                    index += 1;
                    continue;
                }
                if byte == b'\\' {
                    escaped = true;
                    index += 1;
                    continue;
                }
                if byte == delimiter {
                    string_delimiter = None;
                }
                index += 1;
                continue;
            }

            if index + 1 < bytes.len() && byte == b'/' && bytes[index + 1] == b'/' {
                break;
            }
            if index + 1 < bytes.len() && byte == b'/' && bytes[index + 1] == b'*' {
                in_block_comment = true;
                index += 2;
                continue;
            }

            match byte {
                b'"' | b'\'' | b'`' => string_delimiter = Some(byte),
                b'{' => stack.push(line_index),
                b'}' => {
                    let Some(open_line) = stack.pop() else {
                        index += 1;
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
            index += 1;
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
    let start = if anchor_indent == 0 {
        anchor_index
    } else {
        find_indent_parent(lines, anchor_index, anchor_indent).or_else(|| {
            extension(path)
                .is_some_and(|extension| extension == "verse")
                .then_some(0)
        })?
    };
    let start_indent = indent_width(lines[start]);
    let end = find_indent_block_end(lines, start, start_indent, &["#"]);

    Some((start + 1, end + 1))
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
    let start = if anchor_indent == 0 {
        anchor_index
    } else {
        find_indent_parent(lines, anchor_index, anchor_indent).unwrap_or(anchor_index)
    };
    let end = find_indent_block_end(lines, start, indent_width(lines[start]), &["#", "//"]);

    (start + 1, end + 1)
}

fn find_indent_parent(lines: &[&str], anchor_index: usize, anchor_indent: usize) -> Option<usize> {
    (0..anchor_index).rev().find(|index| {
        let line = lines[*index];
        !line.trim().is_empty() && indent_width(line) < anchor_indent
    })
}

fn find_indent_block_end(
    lines: &[&str],
    start_index: usize,
    start_indent: usize,
    comment_prefixes: &[&str],
) -> usize {
    let mut end = lines.len().saturating_sub(1);
    for (index, line) in lines.iter().enumerate().skip(start_index + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || comment_prefixes
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        {
            continue;
        }
        if indent_width(line) <= start_indent {
            end = index.saturating_sub(1);
            break;
        }
    }
    end
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
                | "dart"
                | "go"
                | "h"
                | "hpp"
                | "java"
                | "js"
                | "jsx"
                | "kt"
                | "kts"
                | "m"
                | "mm"
                | "rs"
                | "scss"
                | "scala"
                | "swift"
                | "ts"
                | "tsx"
                | "zig"
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
