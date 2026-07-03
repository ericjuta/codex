use crate::function_tool::FunctionCallError;
use serde::Serialize;

use super::hashline_format::split_lines_preserve;
use super::hashline_hash::hash_hex;
use super::hashline_hash::line_hash;

const APPLY_PATCH_CONTEXT_LINES: usize = 3;
const PATCH_PREVIEW_MAX_LINES: usize = 40;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct HashlinePatchPreview {
    pub(super) old_start_line: Option<usize>,
    pub(super) old_end_line: Option<usize>,
    pub(super) new_start_line: Option<usize>,
    pub(super) new_end_line: Option<usize>,
    pub(super) truncated: bool,
    pub(super) content: String,
}

#[derive(Clone, Copy)]
struct ChangeBounds {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

pub(super) fn apply_hashline_patch(
    path: &str,
    contents: &str,
    patch: &str,
) -> Result<String, FunctionCallError> {
    validate_patch_headers(path, contents, patch)?;

    let mut lines = split_lines_preserve(contents)
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut saw_operation = false;

    for raw_line in patch.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.starts_with('[') || line.starts_with('#') {
            continue;
        }
        saw_operation = true;
        let (op, rest) = split_hashline_operation(line)?;
        match op {
            "SWAP" => {
                let (line_number, expected_hash, replacement) =
                    parse_line_op(rest, /*needs_text*/ true)?;
                validate_line_hash(&lines, line_number, expected_hash)?;
                lines[line_number - 1] = replacement.to_string();
            }
            "DEL" => {
                let (line_number, expected_hash, _) =
                    parse_line_op(rest, /*needs_text*/ false)?;
                validate_line_hash(&lines, line_number, expected_hash)?;
                lines.remove(line_number - 1);
            }
            "INS.PRE" => {
                let (line_number, expected_hash, inserted) =
                    parse_line_op(rest, /*needs_text*/ true)?;
                validate_line_hash(&lines, line_number, expected_hash)?;
                lines.insert(line_number - 1, inserted.to_string());
            }
            "INS.POST" => {
                let (line_number, expected_hash, inserted) =
                    parse_line_op(rest, /*needs_text*/ true)?;
                validate_line_hash(&lines, line_number, expected_hash)?;
                lines.insert(line_number, inserted.to_string());
            }
            "INS.HEAD" => {
                let inserted = parse_insert_text(rest)?;
                lines.insert(0, inserted.to_string());
            }
            "INS.TAIL" => {
                let inserted = parse_insert_text(rest)?;
                lines.push(inserted.to_string());
            }
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "unsupported Hashline operation {op}"
                )));
            }
        }
    }

    if !saw_operation {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not contain any operations".to_string(),
        ));
    }

    let mut output = lines.join("\n");
    if contents.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

pub(super) fn parse_anchor_line(anchor: &str) -> Result<usize, FunctionCallError> {
    let line = anchor
        .split_once(':')
        .map_or(anchor, |(line, _)| line)
        .trim();
    line.parse::<usize>().map_err(|err| {
        FunctionCallError::RespondToModel(format!("invalid Hashline anchor {anchor}: {err}"))
    })
}

pub(super) fn parse_anchor_hash(anchor: &str) -> Option<&str> {
    anchor
        .split_once(':')
        .map(|(_, hash)| hash)
        .filter(|hash| !hash.is_empty())
}

pub(super) fn validate_file_hash(
    path: &str,
    contents: &str,
    expected_hash: Option<&str>,
) -> Result<(), FunctionCallError> {
    if let Some(expected_hash) = expected_hash {
        let actual_hash = hash_hex(contents, 4);
        if expected_hash != actual_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "file hash mismatch for {path}: expected {expected_hash}, found {actual_hash}"
            )));
        }
    }
    Ok(())
}

fn validate_patch_headers(
    path: &str,
    contents: &str,
    patch: &str,
) -> Result<(), FunctionCallError> {
    for raw_line in patch.lines() {
        let line = raw_line.trim();
        if !line.starts_with('[') {
            continue;
        }
        let Some(header) = line
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        else {
            return Err(FunctionCallError::RespondToModel(format!(
                "invalid Hashline file header {line}; expected [{path}#HASH]"
            )));
        };
        let Some((header_path, expected_hash)) = header.rsplit_once('#') else {
            return Err(FunctionCallError::RespondToModel(format!(
                "invalid Hashline file header {line}; expected [{path}#HASH]"
            )));
        };
        if header_path != path {
            return Err(FunctionCallError::RespondToModel(format!(
                "Hashline file header path {header_path} does not match target path {path}"
            )));
        }
        validate_hash_token(path, expected_hash)?;
        validate_file_hash(path, contents, Some(&expected_hash.to_ascii_lowercase()))?;
    }
    Ok(())
}

fn validate_hash_token(path: &str, expected_hash: &str) -> Result<(), FunctionCallError> {
    if expected_hash.len() == 4 && expected_hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(FunctionCallError::RespondToModel(format!(
        "invalid file hash for {path}: expected a 4-hex Hashline file hash, got {expected_hash}"
    )))
}

pub(super) fn ensure_rename_representable(
    path: &str,
    contents: &str,
) -> Result<(), FunctionCallError> {
    if split_lines_preserve(contents).is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.rename_file cannot move empty file {path} through apply_patch"
        )));
    }
    if !contents.ends_with('\n') {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.rename_file cannot preserve non-newline-terminated file {path} through apply_patch"
        )));
    }
    Ok(())
}

pub(super) fn apply_patch_for_hashline_update(
    path: &str,
    old_contents: &str,
    new_contents: &str,
    create: bool,
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let mut patch = String::from("*** Begin Patch\n");
    if let Some(environment_id) = environment_id {
        patch.push_str("*** Environment ID: ");
        patch.push_str(environment_id);
        patch.push('\n');
    }

    if create {
        let new_lines = split_lines_preserve(new_contents);
        if new_lines.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "hashline.patch create=true produced an empty file, which apply_patch Add File cannot represent".to_string(),
            ));
        }
        patch.push_str("*** Add File: ");
        patch.push_str(path);
        patch.push('\n');
        for line in new_lines {
            patch.push('+');
            patch.push_str(line);
            patch.push('\n');
        }
    } else {
        append_localized_update_hunk(&mut patch, path, old_contents, new_contents)?;
    }
    patch.push_str("*** End Patch");
    Ok(patch)
}

pub(super) fn apply_patch_for_hashline_remove(path: &str, environment_id: Option<&str>) -> String {
    let mut patch = apply_patch_header(environment_id);
    patch.push_str("*** Delete File: ");
    patch.push_str(path);
    patch.push('\n');
    patch.push_str("*** End Patch");
    patch
}

pub(super) fn apply_patch_for_hashline_rename(
    path: &str,
    new_path: &str,
    contents: &str,
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let Some(first_line) = split_lines_preserve(contents).first().copied() else {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.rename_file cannot move empty file {path} through apply_patch"
        )));
    };
    let mut patch = apply_patch_header(environment_id);
    patch.push_str("*** Update File: ");
    patch.push_str(path);
    patch.push_str("\n*** Move to: ");
    patch.push_str(new_path);
    patch.push_str("\n@@\n");
    append_apply_patch_line(&mut patch, ' ', first_line);
    patch.push_str("*** End Patch");
    Ok(patch)
}

pub(super) fn build_hashline_patch_preview(
    old_contents: &str,
    new_contents: &str,
) -> Result<HashlinePatchPreview, FunctionCallError> {
    let old_lines = split_lines_preserve(old_contents);
    let new_lines = split_lines_preserve(new_contents);
    let bounds = change_bounds(&old_lines, &new_lines);
    if bounds.old_start == bounds.old_end && bounds.new_start == bounds.new_end {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not change file contents".to_string(),
        ));
    }

    let mut content = Vec::new();
    let mut seen = 0;
    for (line_number, line) in old_lines[bounds.old_start..bounds.old_end]
        .iter()
        .enumerate()
    {
        seen += 1;
        if content.len() < PATCH_PREVIEW_MAX_LINES {
            let line_number = bounds.old_start + line_number + 1;
            content.push(format!("-{line_number}:{}|{line}", line_hash(line)));
        }
    }
    for (line_number, line) in new_lines[bounds.new_start..bounds.new_end]
        .iter()
        .enumerate()
    {
        seen += 1;
        if content.len() < PATCH_PREVIEW_MAX_LINES {
            let line_number = bounds.new_start + line_number + 1;
            content.push(format!("+{line_number}:{}|{line}", line_hash(line)));
        }
    }

    Ok(HashlinePatchPreview {
        old_start_line: span_start(bounds.old_start, bounds.old_end),
        old_end_line: span_end(bounds.old_start, bounds.old_end),
        new_start_line: span_start(bounds.new_start, bounds.new_end),
        new_end_line: span_end(bounds.new_start, bounds.new_end),
        truncated: seen > PATCH_PREVIEW_MAX_LINES,
        content: content.join("\n"),
    })
}

fn parse_line_op(
    input: &str,
    needs_text: bool,
) -> Result<(usize, Option<&str>, &str), FunctionCallError> {
    let (anchor, text) = if let Some((anchor, text)) = input.split_once('|') {
        (anchor.trim(), text)
    } else if needs_text {
        return Err(FunctionCallError::RespondToModel(format!(
            "Hashline operation {input} must include |text"
        )));
    } else {
        (input.trim(), "")
    };
    let line_number = parse_anchor_line(anchor)?;
    let expected_hash = parse_anchor_hash(anchor);
    Ok((line_number, expected_hash, text))
}

fn split_hashline_operation(input: &str) -> Result<(&str, &str), FunctionCallError> {
    let line = input.trim_start();
    let Some(op_end) = line.find(|ch: char| ch.is_whitespace() || ch == '|') else {
        return Err(invalid_operation_error(line));
    };
    let (op, rest) = line.split_at(op_end);
    if op.is_empty() {
        return Err(invalid_operation_error(line));
    }
    let rest = if rest.starts_with('|') {
        rest
    } else {
        rest.trim_start()
    };
    if rest.is_empty() {
        return Err(invalid_operation_error(line));
    }
    Ok((op, rest))
}

fn invalid_operation_error(line: &str) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "invalid Hashline operation {line}; expected forms like SWAP 12:ab|text, DEL 12:ab, INS.POST 12:ab|text, or INS.TAIL |text"
    ))
}

fn parse_insert_text(input: &str) -> Result<&str, FunctionCallError> {
    input.trim_start().strip_prefix('|').ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("insert operation {input} must include |text"))
    })
}

fn validate_line_hash(
    lines: &[String],
    line_number: usize,
    expected_hash: Option<&str>,
) -> Result<(), FunctionCallError> {
    if line_number == 0 || line_number > lines.len() {
        return Err(FunctionCallError::RespondToModel(format!(
            "line {line_number} is outside file range 1..={}",
            lines.len()
        )));
    }
    if let Some(expected_hash) = expected_hash {
        let actual_hash = line_hash(&lines[line_number - 1]);
        if expected_hash != actual_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "line {line_number} hash mismatch: expected {expected_hash}, found {actual_hash}"
            )));
        }
    }
    Ok(())
}

fn apply_patch_header(environment_id: Option<&str>) -> String {
    let mut patch = String::from("*** Begin Patch\n");
    if let Some(environment_id) = environment_id {
        patch.push_str("*** Environment ID: ");
        patch.push_str(environment_id);
        patch.push('\n');
    }
    patch
}

fn append_localized_update_hunk(
    patch: &mut String,
    path: &str,
    old_contents: &str,
    new_contents: &str,
) -> Result<(), FunctionCallError> {
    let old_lines = split_lines_preserve(old_contents);
    let new_lines = split_lines_preserve(new_contents);
    let bounds = change_bounds(&old_lines, &new_lines);
    if bounds.old_start == bounds.old_end && bounds.new_start == bounds.new_end {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not change file contents".to_string(),
        ));
    }

    let context_start = bounds.old_start.saturating_sub(APPLY_PATCH_CONTEXT_LINES);
    let old_context_end = old_lines
        .len()
        .min(bounds.old_end.saturating_add(APPLY_PATCH_CONTEXT_LINES));

    patch.push_str("*** Update File: ");
    patch.push_str(path);
    patch.push_str("\n@@\n");
    for line in &old_lines[context_start..bounds.old_start] {
        append_apply_patch_line(patch, ' ', line);
    }
    for line in &old_lines[bounds.old_start..bounds.old_end] {
        append_apply_patch_line(patch, '-', line);
    }
    for line in &new_lines[bounds.new_start..bounds.new_end] {
        append_apply_patch_line(patch, '+', line);
    }
    for line in &old_lines[bounds.old_end..old_context_end] {
        append_apply_patch_line(patch, ' ', line);
    }
    if !old_contents.ends_with('\n') && old_context_end == old_lines.len() {
        patch.push_str("*** End of File\n");
    }
    Ok(())
}

fn append_apply_patch_line(patch: &mut String, prefix: char, line: &str) {
    patch.push(prefix);
    patch.push_str(line);
    patch.push('\n');
}

fn change_bounds(old_lines: &[&str], new_lines: &[&str]) -> ChangeBounds {
    let common_prefix = old_lines
        .iter()
        .zip(new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let remaining_old = old_lines.len().saturating_sub(common_prefix);
    let remaining_new = new_lines.len().saturating_sub(common_prefix);
    let common_suffix = old_lines[common_prefix..]
        .iter()
        .rev()
        .zip(new_lines[common_prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count()
        .min(remaining_old)
        .min(remaining_new);

    ChangeBounds {
        old_start: common_prefix,
        old_end: old_lines.len() - common_suffix,
        new_start: common_prefix,
        new_end: new_lines.len() - common_suffix,
    }
}

fn span_start(start: usize, end: usize) -> Option<usize> {
    (start < end).then_some(start + 1)
}

fn span_end(start: usize, end: usize) -> Option<usize> {
    (start < end).then_some(end)
}
