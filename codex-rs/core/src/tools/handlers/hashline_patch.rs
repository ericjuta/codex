use crate::function_tool::FunctionCallError;
use serde::Serialize;

use super::hashline_block::find_block_span;
use super::hashline_format::split_lines_preserve;
use super::hashline_hash::hash_hex;
use super::hashline_hash::line_hash;
use super::hashline_hash::normalize_file_text;

const APPLY_PATCH_CONTEXT_LINES: usize = 3;
const PATCH_PREVIEW_MAX_LINES: usize = 40;
const BARE_LINE_ANCHOR_WARNING: &str = "hashline.patch used bare line anchors; prefer line:hash anchors from hashline.read when editing existing files";

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct HashlinePatchPreview {
    pub(super) old_start_line: Option<usize>,
    pub(super) old_end_line: Option<usize>,
    pub(super) new_start_line: Option<usize>,
    pub(super) new_end_line: Option<usize>,
    pub(super) truncated: bool,
    pub(super) content: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct HashlinePatchSection {
    pub(super) path: String,
    pub(super) expected_hash: Option<String>,
    pub(super) patch: String,
}

pub(super) struct HashlinePatchFileUpdate<'a> {
    pub(super) path: &'a str,
    pub(super) old_contents: &'a str,
    pub(super) new_contents: &'a str,
    pub(super) create: bool,
}

pub(super) enum HashlinePatchFileMutation<'a> {
    Update(HashlinePatchFileUpdate<'a>),
    Remove {
        path: &'a str,
    },
    Rename {
        path: &'a str,
        new_path: &'a str,
        old_contents: &'a str,
        new_contents: &'a str,
    },
}

#[derive(Clone, Copy)]
struct ChangeBounds {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

struct PayloadLine {
    text: String,
    kind: PayloadLineKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PayloadLineKind {
    Bare,
    Literal,
}

#[derive(Debug, Clone)]
struct LineAnchor {
    line: usize,
    expected_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct LineRange {
    start: LineAnchor,
    end: LineAnchor,
}

#[derive(Debug, Clone)]
enum HashlineOperation {
    Swap {
        range: LineRange,
        replacement: Vec<String>,
    },
    Delete {
        range: LineRange,
    },
    InsertBefore {
        anchor: LineAnchor,
        inserted: Vec<String>,
    },
    InsertAfter {
        anchor: LineAnchor,
        inserted: Vec<String>,
    },
    InsertHead {
        inserted: Vec<String>,
    },
    InsertTail {
        inserted: Vec<String>,
    },
    SwapBlock {
        anchor: LineAnchor,
        replacement: Vec<String>,
    },
    DeleteBlock {
        anchor: LineAnchor,
    },
    InsertBlockBefore {
        anchor: LineAnchor,
        inserted: Vec<String>,
    },
    InsertBlockAfter {
        anchor: LineAnchor,
        inserted: Vec<String>,
    },
    RemoveFile,
    RenameFile {
        new_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HashlinePatchFileOperation {
    Remove,
    Rename { new_path: String },
}

pub(super) fn apply_hashline_patch(
    path: &str,
    contents: &str,
    patch: &str,
) -> Result<String, FunctionCallError> {
    if hashline_patch_is_aborted(patch) {
        return Ok(contents.to_string());
    }
    validate_patch_headers(path, contents, patch)?;
    let operations = parse_hashline_patch(patch)?;
    if operations
        .iter()
        .any(|operation| matches!(operation, HashlineOperation::RemoveFile))
    {
        return Err(FunctionCallError::RespondToModel(
            "REM is a file operation; use a sectioned hashline.patch or the dedicated hashline file tools"
                .to_string(),
        ));
    }
    let operations = operations
        .into_iter()
        .filter(|operation| !operation.is_file_operation())
        .collect::<Vec<_>>();
    let contents_bytes = contents.as_bytes();
    let restore_crlf = contents_bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .is_some_and(|index| index > 0 && contents_bytes[index - 1] == b'\r');
    let normalized_contents = normalize_file_text(contents);
    let mut lines = split_lines_preserve(&normalized_contents)
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if operations.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not contain any operations".to_string(),
        ));
    }

    apply_operations(path, &mut lines, &operations, &normalized_contents)?;

    let mut output = lines.join("\n");
    if normalized_contents.ends_with('\n') {
        output.push('\n');
    }
    if restore_crlf {
        output = output.replace('\n', "\r\n");
    }
    Ok(output)
}

pub(super) fn parse_hashline_patch_file_operation(
    patch: &str,
) -> Result<Option<HashlinePatchFileOperation>, FunctionCallError> {
    let operations = parse_hashline_patch(patch)?;
    let mut file_operation = None;
    let mut has_line_operation = false;
    for operation in operations {
        match operation {
            HashlineOperation::RemoveFile => {
                set_file_operation(&mut file_operation, HashlinePatchFileOperation::Remove)?;
            }
            HashlineOperation::RenameFile { new_path } => {
                set_file_operation(
                    &mut file_operation,
                    HashlinePatchFileOperation::Rename { new_path },
                )?;
            }
            HashlineOperation::Swap { .. }
            | HashlineOperation::Delete { .. }
            | HashlineOperation::InsertBefore { .. }
            | HashlineOperation::InsertAfter { .. }
            | HashlineOperation::InsertHead { .. }
            | HashlineOperation::InsertTail { .. }
            | HashlineOperation::SwapBlock { .. }
            | HashlineOperation::DeleteBlock { .. }
            | HashlineOperation::InsertBlockBefore { .. }
            | HashlineOperation::InsertBlockAfter { .. } => {
                has_line_operation = true;
            }
        }
    }
    if matches!(file_operation, Some(HashlinePatchFileOperation::Remove)) && has_line_operation {
        return Err(FunctionCallError::RespondToModel(
            "Hashline file operation REM cannot be combined with line operations in the same file section"
                .to_string(),
        ));
    }
    Ok(file_operation)
}

pub(super) fn hashline_patch_has_line_operations(patch: &str) -> Result<bool, FunctionCallError> {
    let operations = parse_hashline_patch(patch)?;
    Ok(operations
        .iter()
        .any(|operation| !operation.is_file_operation()))
}

fn set_file_operation(
    file_operation: &mut Option<HashlinePatchFileOperation>,
    next_operation: HashlinePatchFileOperation,
) -> Result<(), FunctionCallError> {
    if let Some(previous_operation) = file_operation {
        return Err(FunctionCallError::RespondToModel(format!(
            "only one Hashline file operation is allowed per file section; found {previous_operation:?} and {next_operation:?}"
        )));
    }
    *file_operation = Some(next_operation);
    Ok(())
}

impl HashlineOperation {
    fn is_file_operation(&self) -> bool {
        matches!(
            self,
            HashlineOperation::RemoveFile | HashlineOperation::RenameFile { .. }
        )
    }

    fn uses_bare_line_anchor(&self) -> bool {
        match self {
            HashlineOperation::Swap { range, .. } | HashlineOperation::Delete { range } => {
                range.start.expected_hash.is_none() || range.end.expected_hash.is_none()
            }
            HashlineOperation::InsertBefore { anchor, .. }
            | HashlineOperation::InsertAfter { anchor, .. }
            | HashlineOperation::SwapBlock { anchor, .. }
            | HashlineOperation::DeleteBlock { anchor }
            | HashlineOperation::InsertBlockBefore { anchor, .. }
            | HashlineOperation::InsertBlockAfter { anchor, .. } => anchor.expected_hash.is_none(),
            HashlineOperation::InsertHead { .. }
            | HashlineOperation::InsertTail { .. }
            | HashlineOperation::RemoveFile
            | HashlineOperation::RenameFile { .. } => false,
        }
    }
}

pub(super) fn hashline_patch_warnings(patch: &str) -> Result<Vec<String>, FunctionCallError> {
    let operations = parse_hashline_patch(patch)?;
    if operations
        .iter()
        .any(HashlineOperation::uses_bare_line_anchor)
    {
        Ok(vec![BARE_LINE_ANCHOR_WARNING.to_string()])
    } else {
        Ok(Vec::new())
    }
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

pub(super) fn split_hashline_patch_sections(
    default_path: &str,
    patch: &str,
) -> Result<Vec<HashlinePatchSection>, FunctionCallError> {
    let mut has_header = false;
    for line in patch.lines() {
        if parse_patch_file_header(default_path, line)?.is_some() {
            has_header = true;
            break;
        }
    }
    if !has_header {
        return Ok(vec![HashlinePatchSection {
            path: default_path.to_string(),
            expected_hash: None,
            patch: patch.to_string(),
        }]);
    }

    let mut sections = Vec::<HashlinePatchSection>::new();
    let mut current_index = None;
    for raw_line in patch.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some((path, expected_hash)) = parse_patch_file_header(default_path, line)? {
            let section_index = match sections.iter().position(|section| section.path == path) {
                Some(index) => {
                    merge_section_hash(&mut sections[index], expected_hash)?;
                    index
                }
                None => {
                    sections.push(HashlinePatchSection {
                        path,
                        expected_hash,
                        patch: String::new(),
                    });
                    sections.len() - 1
                }
            };
            current_index = Some(section_index);
            continue;
        }

        let Some(section_index) = current_index else {
            if is_ignorable_patch_line(line) {
                continue;
            }
            if let Some(message) = apply_patch_contamination_message(line) {
                return Err(FunctionCallError::RespondToModel(message));
            }
            return Err(FunctionCallError::RespondToModel(format!(
                "Hashline operation {line:?} appears before the first [path#HASH] section header"
            )));
        };

        if !sections[section_index].patch.is_empty() {
            sections[section_index].patch.push('\n');
        }
        sections[section_index].patch.push_str(line);
    }

    if sections.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not contain any file sections".to_string(),
        ));
    }
    Ok(sections)
}

fn parse_patch_file_header(
    target_path: &str,
    line: &str,
) -> Result<Option<(String, Option<String>)>, FunctionCallError> {
    let line = line.trim();
    if !line.starts_with('[') {
        return Ok(None);
    }
    let Some(header) = line
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline file header {line}; expected [{target_path}#HASH]"
        )));
    };
    let (header_path, expected_hash) = match header.rsplit_once('#') {
        Some((header_path, expected_hash)) => {
            validate_hash_token(header_path, expected_hash)?;
            (header_path, Some(expected_hash.to_ascii_lowercase()))
        }
        None => (header, None),
    };
    let header_path = strip_apply_patch_path_noise(header_path);
    if header_path.trim().is_empty() || header_path.contains('#') {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline file header {line}; expected [{target_path}#HASH] or [{target_path}]"
        )));
    }
    Ok(Some((header_path, expected_hash)))
}

fn strip_apply_patch_path_noise(path_text: &str) -> String {
    let bytes = path_text.as_bytes();
    let mut stripped_stars = 0;
    while stripped_stars < bytes.len() && stripped_stars < 3 && bytes[stripped_stars] == b'*' {
        stripped_stars += 1;
    }

    let stripped = path_text[stripped_stars..].trim_start();
    let stripped_lower = stripped.to_ascii_lowercase();
    for keyword in ["update", "delete", "add", "move"] {
        if !stripped_lower.starts_with(keyword) {
            continue;
        }
        let after_keyword = &stripped[keyword.len()..];
        let after_separator =
            after_keyword.trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != ':');
        let after_separator_lower = after_separator.to_ascii_lowercase();
        let after_optional_word = if after_separator_lower.starts_with("file") {
            after_separator["file".len()..]
                .trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != ':')
        } else if after_separator_lower.starts_with("to") {
            after_separator["to".len()..]
                .trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != ':')
        } else {
            after_separator
        };
        if let Some(path) = after_optional_word.strip_prefix(':') {
            return path.trim_start().to_string();
        }
    }

    stripped.to_string()
}

fn merge_section_hash(
    section: &mut HashlinePatchSection,
    expected_hash: Option<String>,
) -> Result<(), FunctionCallError> {
    let Some(expected_hash) = expected_hash else {
        return Ok(());
    };
    match &section.expected_hash {
        Some(previous_hash) if previous_hash != &expected_hash => {
            Err(FunctionCallError::RespondToModel(format!(
                "conflicting hash tags for {}: {} vs {}",
                section.path, previous_hash, expected_hash
            )))
        }
        None => {
            section.expected_hash = Some(expected_hash);
            Ok(())
        }
        Some(_) => Ok(()),
    }
}

fn validate_patch_headers(
    path: &str,
    contents: &str,
    patch: &str,
) -> Result<(), FunctionCallError> {
    let mut section_hash = None;
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
        let header_path = strip_apply_patch_path_noise(header_path);
        if header_path != path {
            return Err(FunctionCallError::RespondToModel(format!(
                "Hashline file header path {header_path} does not match target path {path}; this single-file patch application only accepts headers for {path}"
            )));
        }
        validate_hash_token(path, expected_hash)?;
        let expected_hash = expected_hash.to_ascii_lowercase();
        match &section_hash {
            Some(previous_hash) if previous_hash != &expected_hash => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "conflicting hash tags for {path}: {previous_hash} vs {expected_hash}"
                )));
            }
            None => section_hash = Some(expected_hash),
            Some(_) => {}
        }
    }
    if let Some(expected_hash) = section_hash {
        validate_file_hash(path, contents, Some(&expected_hash))?;
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

fn parse_hashline_patch(patch: &str) -> Result<Vec<HashlineOperation>, FunctionCallError> {
    let raw_lines = patch
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>();
    let mut operations = Vec::new();
    let mut index = 0;
    while index < raw_lines.len() {
        let line = raw_lines[index].trim_end();
        if is_ignorable_patch_line(line) {
            index += 1;
            continue;
        }
        if let Some(message) = apply_patch_contamination_message(line) {
            return Err(FunctionCallError::RespondToModel(message));
        }
        let (op, rest) = split_hashline_operation(line)?;
        let op = op.to_ascii_uppercase();
        index += 1;

        let operation = match op.as_str() {
            "SWAP" => {
                if let Some((target, replacement)) = rest.split_once('|') {
                    HashlineOperation::Swap {
                        range: parse_line_range(target)?,
                        replacement: vec![replacement.to_string()],
                    }
                } else {
                    HashlineOperation::Swap {
                        range: parse_line_range(rest)?,
                        replacement: collect_payload_lines(&raw_lines, &mut index)?,
                    }
                }
            }
            "DEL" => HashlineOperation::Delete {
                range: parse_line_range(rest)?,
            },
            "INS.PRE" => {
                if let Some((target, inserted)) = rest.split_once('|') {
                    HashlineOperation::InsertBefore {
                        anchor: parse_line_anchor(target)?,
                        inserted: vec![inserted.to_string()],
                    }
                } else {
                    HashlineOperation::InsertBefore {
                        anchor: parse_line_anchor(rest)?,
                        inserted: collect_payload_lines(&raw_lines, &mut index)?,
                    }
                }
            }
            "INS.POST" => {
                if let Some((target, inserted)) = rest.split_once('|') {
                    HashlineOperation::InsertAfter {
                        anchor: parse_line_anchor(target)?,
                        inserted: vec![inserted.to_string()],
                    }
                } else {
                    HashlineOperation::InsertAfter {
                        anchor: parse_line_anchor(rest)?,
                        inserted: collect_payload_lines(&raw_lines, &mut index)?,
                    }
                }
            }
            "INS.HEAD" => {
                if let Some(inserted) = rest.trim_start().strip_prefix('|') {
                    HashlineOperation::InsertHead {
                        inserted: vec![inserted.to_string()],
                    }
                } else {
                    validate_empty_target(rest, "INS.HEAD")?;
                    HashlineOperation::InsertHead {
                        inserted: collect_payload_lines(&raw_lines, &mut index)?,
                    }
                }
            }
            "INS.TAIL" => {
                if let Some(inserted) = rest.trim_start().strip_prefix('|') {
                    HashlineOperation::InsertTail {
                        inserted: vec![inserted.to_string()],
                    }
                } else {
                    validate_empty_target(rest, "INS.TAIL")?;
                    HashlineOperation::InsertTail {
                        inserted: collect_payload_lines(&raw_lines, &mut index)?,
                    }
                }
            }
            "SWAP.BLK" => HashlineOperation::SwapBlock {
                anchor: parse_line_anchor(rest)?,
                replacement: collect_payload_lines(&raw_lines, &mut index)?,
            },
            "DEL.BLK" => HashlineOperation::DeleteBlock {
                anchor: parse_line_anchor(rest)?,
            },
            "INS.BLK.POST" | "INS.BLK" => HashlineOperation::InsertBlockAfter {
                anchor: parse_line_anchor(rest)?,
                inserted: collect_payload_lines(&raw_lines, &mut index)?,
            },
            "INS.BLK.PRE" => HashlineOperation::InsertBlockBefore {
                anchor: parse_line_anchor(rest)?,
                inserted: collect_payload_lines(&raw_lines, &mut index)?,
            },
            "REM" => {
                validate_empty_target(rest, "REM")?;
                HashlineOperation::RemoveFile
            }
            "MV" => HashlineOperation::RenameFile {
                new_path: parse_move_target(rest)?,
            },
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "unsupported Hashline operation {op}"
                )));
            }
        };
        operations.push(operation);
    }
    Ok(operations)
}

pub(super) fn hashline_patch_is_aborted(patch: &str) -> bool {
    patch
        .lines()
        .any(|line| line.trim_end_matches('\r').trim_end() == "*** Abort")
}

fn is_ignorable_patch_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || trimmed.starts_with('[')
        || trimmed.starts_with('#')
        || trimmed == "*** Begin Patch"
        || trimmed == "*** End Patch"
}

fn collect_payload_lines(
    raw_lines: &[&str],
    index: &mut usize,
) -> Result<Vec<String>, FunctionCallError> {
    let mut payload = Vec::new();
    while *index < raw_lines.len() {
        let line = raw_lines[*index].trim_end();
        if is_ignorable_patch_line(line) {
            *index += 1;
            if payload.is_empty() {
                continue;
            }
            break;
        }
        if let Some(message) = apply_patch_contamination_message(line) {
            return Err(FunctionCallError::RespondToModel(message));
        }
        if is_hashline_operation_line(line) {
            break;
        }
        if line.trim_start().starts_with('-') {
            return Err(FunctionCallError::RespondToModel(format!(
                "Hashline payload line {line:?} must start with + or be a bare replacement line; - rows are not accepted"
            )));
        }
        let (text, kind) = match line.strip_prefix('+') {
            Some(text) => (text, PayloadLineKind::Literal),
            None => (line, PayloadLineKind::Bare),
        };
        payload.push(PayloadLine {
            text: text.to_string(),
            kind,
        });
        *index += 1;
    }
    strip_uniform_read_output_payload_prefixes(&mut payload);
    Ok(payload.into_iter().map(|line| line.text).collect())
}

fn strip_uniform_read_output_payload_prefixes(payload: &mut [PayloadLine]) {
    let is_bare_literal_value = |line: &str| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        if ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
            && trimmed.len() > 2
        {
            return true;
        }
        if let Some(stripped) = trimmed.strip_suffix(',') {
            return stripped.parse::<f64>().is_ok();
        }
        trimmed.parse::<f64>().is_ok()
    };

    let mut saw_bare_payload = false;
    let mut all_literal_values = true;
    for line in payload.iter() {
        if line.kind != PayloadLineKind::Bare || line.text.trim().is_empty() {
            continue;
        }
        saw_bare_payload = true;
        let Some(stripped) = strip_read_output_payload_prefix(&line.text) else {
            return;
        };
        if !is_bare_literal_value(stripped) {
            all_literal_values = false;
        }
    }
    if !saw_bare_payload || all_literal_values {
        return;
    }
    for line in payload.iter_mut() {
        if line.kind == PayloadLineKind::Bare
            && !line.text.trim().is_empty()
            && let Some(stripped) = strip_read_output_payload_prefix(&line.text)
        {
            line.text = stripped.to_string();
        }
    }
}

fn strip_read_output_payload_prefix(line: &str) -> Option<&str> {
    let mut line = line.trim_start_matches([' ', '\t']);
    if let Some(prompt_stripped) = line.strip_prefix(">>>").or_else(|| line.strip_prefix(">>")) {
        line = prompt_stripped.trim_start_matches([' ', '\t']);
    }
    if let Some(marker_stripped) = line.strip_prefix('+').or_else(|| line.strip_prefix('*')) {
        line = marker_stripped.trim_start_matches([' ', '\t']);
    }

    let (line_number, rest) = line.split_once(':')?;
    if line_number.is_empty() || !line_number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let (line_hash, content) = rest.split_once('|')?;
    (line_hash.len() == 2 && line_hash.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(content)
}

fn apply_patch_contamination_message(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("*** Update File:")
        || trimmed.starts_with("*** Add File:")
        || trimmed.starts_with("*** Delete File:")
        || trimmed.starts_with("*** Move to:")
    {
        let preview = if trimmed.chars().count() > 48 {
            format!("{}...", trimmed.chars().take(48).collect::<String>())
        } else {
            trimmed.to_string()
        };
        return Some(format!(
            "apply_patch sentinel {preview:?} is not valid in hashline. File sections start with [path#HASH]. Use SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL, or block ops."
        ));
    }
    if trimmed.starts_with("@@ ") && trimmed.contains("@@") {
        return Some(
            "unified-diff hunk header is not valid in hashline. Use SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL, or block ops."
                .to_string(),
        );
    }
    None
}

fn is_hashline_operation_line(line: &str) -> bool {
    let Ok((op, _)) = split_hashline_operation(line) else {
        return false;
    };
    matches!(
        op.to_ascii_uppercase().as_str(),
        "SWAP"
            | "DEL"
            | "INS.PRE"
            | "INS.POST"
            | "INS.HEAD"
            | "INS.TAIL"
            | "SWAP.BLK"
            | "DEL.BLK"
            | "INS.BLK"
            | "INS.BLK.POST"
            | "INS.BLK.PRE"
            | "REM"
            | "MV"
    )
}

fn validate_empty_target(rest: &str, op: &str) -> Result<(), FunctionCallError> {
    let rest = rest.trim();
    if rest.is_empty() || rest == ":" {
        return Ok(());
    }
    Err(FunctionCallError::RespondToModel(format!(
        "{op} does not accept a line target"
    )))
}

fn parse_move_target(rest: &str) -> Result<String, FunctionCallError> {
    let rest = rest.trim().strip_prefix(':').map_or(rest.trim(), str::trim);
    if rest.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "MV requires a destination path".to_string(),
        ));
    }
    let mut chars = rest.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(FunctionCallError::RespondToModel(
            "MV requires a destination path".to_string(),
        ));
    };
    if first != '\'' && first != '"' {
        return Ok(rest.to_string());
    }

    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == first {
            let after = rest[index + ch.len_utf8()..].trim();
            if !after.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "invalid MV destination {rest:?}: unexpected trailing text {after:?}"
                )));
            }
            let inner = &rest[first.len_utf8()..index];
            if inner.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "MV requires a destination path".to_string(),
                ));
            }
            return Ok(inner.to_string());
        }
    }
    Err(FunctionCallError::RespondToModel(format!(
        "invalid MV destination {rest:?}: missing closing quote"
    )))
}

fn parse_line_range(input: &str) -> Result<LineRange, FunctionCallError> {
    let normalized = normalize_anchor_target(input);
    let (start, end) = if let Some((start, end)) = split_range_anchor_text(&normalized) {
        (
            parse_line_anchor_text(start.trim(), input)?,
            parse_line_anchor_text(end.trim(), input)?,
        )
    } else {
        let anchor = parse_line_anchor_text(&normalized, input)?;
        (anchor.clone(), anchor)
    };
    if end.line < start.line {
        return Err(FunctionCallError::RespondToModel(format!(
            "Hashline range {input} ends before it starts"
        )));
    }
    Ok(LineRange { start, end })
}

fn parse_line_anchor(input: &str) -> Result<LineAnchor, FunctionCallError> {
    let range = parse_line_range(input)?;
    if range.start.line != range.end.line {
        return Err(FunctionCallError::RespondToModel(format!(
            "Hashline insert anchor {input} must be a single line"
        )));
    }
    Ok(range.start)
}

fn normalize_anchor_target(input: &str) -> String {
    let input = input.trim();
    if input.ends_with(':') {
        return input[..input.len() - 1].to_string();
    }
    input.to_string()
}

fn split_range_anchor_text(input: &str) -> Option<(&str, &str)> {
    if let Some(index) = input.find("..=") {
        return Some((&input[..index], &input[index + 3..]));
    }
    if let Some(index) = input.find("..") {
        return Some((&input[..index], &input[index + 2..]));
    }
    input
        .find('-')
        .map(|index| (&input[..index], &input[index + 1..]))
}

fn parse_line_anchor_text(input: &str, source: &str) -> Result<LineAnchor, FunctionCallError> {
    let (line_text, expected_hash) = split_optional_anchor_hash(input.trim(), source)?;
    Ok(LineAnchor {
        line: parse_positive_line_number(line_text, source)?,
        expected_hash,
    })
}

fn split_optional_anchor_hash<'a>(
    input: &'a str,
    source: &str,
) -> Result<(&'a str, Option<String>), FunctionCallError> {
    if source.ends_with("::") {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline anchor {source}: expected formats like 1, 1:ab, or 1-2 hex hash with one optional ':'"
        )));
    }

    if input.is_empty() {
        return Err(invalid_operation_error(source));
    }

    let (target, expected_hash) = match input.rsplit_once(':') {
        Some((target, hash)) => {
            let hash = hash.trim().to_ascii_lowercase();
            if hash.is_empty() {
                (target.trim(), None)
            } else if hash.len() > 2 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "invalid Hashline anchor {source}: expected a 1-2 hex hash token after ':'; got {hash}"
                )));
            } else {
                (target.trim(), Some(hash))
            }
        }
        None => (input, None),
    };
    if target.is_empty() || target.ends_with(':') || target.contains(':') {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline anchor {source}: expected formats like 1, 1:ab, or 1-2 hex hash with one optional ':'"
        )));
    }

    Ok((target, expected_hash))
}

fn parse_positive_line_number(input: &str, source: &str) -> Result<usize, FunctionCallError> {
    let line_number = input.trim().parse::<usize>().map_err(|err| {
        FunctionCallError::RespondToModel(format!("invalid Hashline anchor {source}: {err}"))
    })?;
    if line_number == 0 {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline anchor {source}: line numbers are 1-indexed"
        )));
    }
    Ok(line_number)
}

fn apply_operations(
    path: &str,
    lines: &mut Vec<String>,
    operations: &[HashlineOperation],
    original_contents: &str,
) -> Result<(), FunctionCallError> {
    let original_lines = split_lines_preserve(original_contents)
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut shifts = vec![0_isize; original_lines.len()];
    let mut deleted = vec![false; original_lines.len()];

    for operation in operations {
        match operation {
            HashlineOperation::Swap { range, replacement } => {
                validate_range(&original_lines, &deleted, range)?;
                let start_index = adjusted_index(range.start.line, &shifts)?;
                let replaced_count = range.end.line - range.start.line + 1;
                replace_current_range(lines, start_index, replaced_count, replacement)?;
                mark_deleted(&mut deleted, range.start.line, range.end.line);
                apply_delta_after(
                    &mut shifts,
                    range.end.line,
                    replacement.len() as isize - replaced_count as isize,
                );
            }
            HashlineOperation::Delete { range } => {
                validate_range(&original_lines, &deleted, range)?;
                let start_index = adjusted_index(range.start.line, &shifts)?;
                let deleted_count = range.end.line - range.start.line + 1;
                replace_current_range(lines, start_index, deleted_count, &[])?;
                mark_deleted(&mut deleted, range.start.line, range.end.line);
                apply_delta_after(&mut shifts, range.end.line, -(deleted_count as isize));
            }
            HashlineOperation::InsertBefore { anchor, inserted } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let index = adjusted_index(anchor.line, &shifts)?;
                for (offset, line) in inserted.iter().enumerate() {
                    lines.insert(index + offset, line.clone());
                }
                apply_delta_from(&mut shifts, anchor.line, inserted.len() as isize);
            }
            HashlineOperation::InsertAfter { anchor, inserted } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let index = adjusted_index(anchor.line, &shifts)?.saturating_add(1);
                for (offset, line) in inserted.iter().enumerate() {
                    lines.insert(index + offset, line.clone());
                }
                apply_delta_after(&mut shifts, anchor.line, inserted.len() as isize);
            }
            HashlineOperation::InsertHead { inserted } => {
                for (offset, line) in inserted.iter().enumerate() {
                    lines.insert(offset, line.clone());
                }
                apply_delta_from(&mut shifts, 1, inserted.len() as isize);
            }
            HashlineOperation::InsertTail { inserted } => {
                lines.extend(inserted.iter().cloned());
            }
            HashlineOperation::SwapBlock {
                anchor,
                replacement,
            } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let original_span = block_span(path, &original_lines, anchor.line)?;
                validate_not_deleted(&deleted, original_span.0, original_span.1)?;
                let current_line = adjusted_index(anchor.line, &shifts)?.saturating_add(1);
                let current_span = block_span(path, lines, current_line)?;
                replace_current_range(
                    lines,
                    current_span.0 - 1,
                    current_span.1 - current_span.0 + 1,
                    replacement,
                )?;
                mark_deleted(&mut deleted, original_span.0, original_span.1);
                apply_delta_after(
                    &mut shifts,
                    original_span.1,
                    replacement.len() as isize - (original_span.1 - original_span.0 + 1) as isize,
                );
            }
            HashlineOperation::DeleteBlock { anchor } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let original_span = block_span(path, &original_lines, anchor.line)?;
                validate_not_deleted(&deleted, original_span.0, original_span.1)?;
                let current_line = adjusted_index(anchor.line, &shifts)?.saturating_add(1);
                let current_span = block_span(path, lines, current_line)?;
                replace_current_range(
                    lines,
                    current_span.0 - 1,
                    current_span.1 - current_span.0 + 1,
                    &[],
                )?;
                mark_deleted(&mut deleted, original_span.0, original_span.1);
                apply_delta_after(
                    &mut shifts,
                    original_span.1,
                    -((original_span.1 - original_span.0 + 1) as isize),
                );
            }
            HashlineOperation::InsertBlockBefore { anchor, inserted } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let original_span = block_span(path, &original_lines, anchor.line)?;
                validate_not_deleted(&deleted, original_span.0, original_span.1)?;
                let current_line = adjusted_index(anchor.line, &shifts)?.saturating_add(1);
                let current_span = block_span(path, lines, current_line)?;
                for (offset, line) in inserted.iter().enumerate() {
                    lines.insert(current_span.0 - 1 + offset, line.clone());
                }
                apply_delta_from(&mut shifts, original_span.0, inserted.len() as isize);
            }
            HashlineOperation::InsertBlockAfter { anchor, inserted } => {
                validate_anchor(&original_lines, &deleted, anchor)?;
                let original_span = block_span(path, &original_lines, anchor.line)?;
                validate_not_deleted(&deleted, original_span.0, original_span.1)?;
                let current_line = adjusted_index(anchor.line, &shifts)?.saturating_add(1);
                let current_span = block_span(path, lines, current_line)?;
                for (offset, line) in inserted.iter().enumerate() {
                    lines.insert(current_span.1 + offset, line.clone());
                }
                apply_delta_after(&mut shifts, original_span.1, inserted.len() as isize);
            }
            HashlineOperation::RemoveFile | HashlineOperation::RenameFile { .. } => {
                return Err(FunctionCallError::RespondToModel(
                    "REM and MV are file operations, not line operations".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn block_span(
    path: &str,
    lines: &[String],
    anchor_line: usize,
) -> Result<(usize, usize), FunctionCallError> {
    if anchor_line == 0 || anchor_line > lines.len().max(1) {
        return Err(FunctionCallError::RespondToModel(format!(
            "block anchor line {anchor_line} is outside file range 1..={}",
            lines.len()
        )));
    }
    let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
    let (start, mut end) = find_block_span(path, &refs, anchor_line);
    while end > start && refs[end - 1].trim().is_empty() {
        end -= 1;
    }
    Ok((start, end))
}

fn validate_range(
    original_lines: &[String],
    deleted: &[bool],
    range: &LineRange,
) -> Result<(), FunctionCallError> {
    validate_anchor(original_lines, deleted, &range.start)?;
    if range.end.line > original_lines.len() {
        return Err(FunctionCallError::RespondToModel(format!(
            "line {} is outside file range 1..={}",
            range.end.line,
            original_lines.len()
        )));
    }
    for line in range.start.line..=range.end.line {
        validate_not_deleted(deleted, line, line)?;
    }
    if range.end.line != range.start.line {
        validate_line_hash(
            original_lines,
            range.end.line,
            range.end.expected_hash.as_deref(),
        )?;
    }
    Ok(())
}

fn validate_not_deleted(
    deleted: &[bool],
    start_line: usize,
    end_line: usize,
) -> Result<(), FunctionCallError> {
    for line in start_line..=end_line {
        if deleted[line - 1] {
            return Err(FunctionCallError::RespondToModel(format!(
                "line {line} has already been deleted by an earlier Hashline operation"
            )));
        }
    }
    Ok(())
}

fn validate_anchor(
    original_lines: &[String],
    deleted: &[bool],
    anchor: &LineAnchor,
) -> Result<(), FunctionCallError> {
    validate_line_hash(original_lines, anchor.line, anchor.expected_hash.as_deref())?;
    if deleted[anchor.line - 1] {
        return Err(FunctionCallError::RespondToModel(format!(
            "line {} has already been deleted by an earlier Hashline operation",
            anchor.line
        )));
    }
    Ok(())
}

fn adjusted_index(line_number: usize, shifts: &[isize]) -> Result<usize, FunctionCallError> {
    let shift = shifts.get(line_number - 1).copied().ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
            "line {line_number} is outside file range 1..={}",
            shifts.len()
        ))
    })?;
    let index = (line_number as isize - 1) + shift;
    usize::try_from(index).map_err(|_| {
        FunctionCallError::RespondToModel(format!(
            "line {line_number} has shifted before the start of the file"
        ))
    })
}

fn replace_current_range(
    lines: &mut Vec<String>,
    start_index: usize,
    removed_count: usize,
    inserted: &[String],
) -> Result<(), FunctionCallError> {
    if start_index.saturating_add(removed_count) > lines.len() {
        return Err(FunctionCallError::RespondToModel(
            "Hashline operation no longer maps to the current file contents".to_string(),
        ));
    }
    for _ in 0..removed_count {
        lines.remove(start_index);
    }
    for (offset, line) in inserted.iter().enumerate() {
        lines.insert(start_index + offset, line.clone());
    }
    Ok(())
}

fn mark_deleted(deleted: &mut [bool], start_line: usize, end_line: usize) {
    for line in start_line..=end_line {
        deleted[line - 1] = true;
    }
}

fn apply_delta_from(shifts: &mut [isize], start_line: usize, delta: isize) {
    for shift in shifts.iter_mut().skip(start_line.saturating_sub(1)) {
        *shift += delta;
    }
}

fn apply_delta_after(shifts: &mut [isize], line_number: usize, delta: isize) {
    for shift in shifts.iter_mut().skip(line_number) {
        *shift += delta;
    }
}

pub(super) fn apply_patch_for_hashline_update(
    path: &str,
    old_contents: &str,
    new_contents: &str,
    create: bool,
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    apply_patch_for_hashline_updates(
        &[HashlinePatchFileUpdate {
            path,
            old_contents,
            new_contents,
            create,
        }],
        environment_id,
    )
}

pub(super) fn apply_patch_for_hashline_updates(
    updates: &[HashlinePatchFileUpdate<'_>],
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let mutations = updates
        .iter()
        .map(|update| {
            HashlinePatchFileMutation::Update(HashlinePatchFileUpdate {
                path: update.path,
                old_contents: update.old_contents,
                new_contents: update.new_contents,
                create: update.create,
            })
        })
        .collect::<Vec<_>>();
    apply_patch_for_hashline_mutations(&mutations, environment_id)
}

pub(super) fn apply_patch_for_hashline_mutations(
    mutations: &[HashlinePatchFileMutation<'_>],
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let mut patch = apply_patch_header(environment_id);
    for mutation in mutations {
        match mutation {
            HashlinePatchFileMutation::Update(update) => {
                append_hashline_update_hunk(&mut patch, update)?;
            }
            HashlinePatchFileMutation::Remove { path } => {
                append_hashline_remove_hunk(&mut patch, path);
            }
            HashlinePatchFileMutation::Rename {
                path,
                new_path,
                old_contents,
                new_contents,
            } => {
                append_hashline_rename_hunk(
                    &mut patch,
                    path,
                    new_path,
                    old_contents,
                    new_contents,
                )?;
            }
        }
    }
    patch.push_str("*** End Patch");
    Ok(patch)
}

fn append_hashline_update_hunk(
    patch: &mut String,
    update: &HashlinePatchFileUpdate<'_>,
) -> Result<(), FunctionCallError> {
    if update.create {
        let new_lines = split_lines_preserve(update.new_contents);
        patch.push_str("*** Add File: ");
        patch.push_str(update.path);
        patch.push('\n');
        for line in new_lines {
            patch.push('+');
            patch.push_str(line);
            patch.push('\n');
        }
    } else {
        append_localized_update_hunk(patch, update.path, update.old_contents, update.new_contents)?;
    }
    Ok(())
}

pub(super) fn apply_patch_for_hashline_remove(path: &str, environment_id: Option<&str>) -> String {
    let mut patch = apply_patch_header(environment_id);
    append_hashline_remove_hunk(&mut patch, path);
    patch.push_str("*** End Patch");
    patch
}

pub(super) fn apply_patch_for_hashline_rename(
    path: &str,
    new_path: &str,
    _contents: &str,
    environment_id: Option<&str>,
) -> String {
    let mut patch = apply_patch_header(environment_id);
    append_hashline_rename_only_hunk(&mut patch, path, new_path);
    patch.push_str("*** End Patch");
    patch
}

fn append_hashline_remove_hunk(patch: &mut String, path: &str) {
    patch.push_str("*** Delete File: ");
    patch.push_str(path);
    patch.push('\n');
}

fn append_hashline_rename_hunk(
    patch: &mut String,
    path: &str,
    new_path: &str,
    old_contents: &str,
    new_contents: &str,
) -> Result<(), FunctionCallError> {
    if old_contents == new_contents {
        append_hashline_rename_only_hunk(patch, path, new_path);
        return Ok(());
    }
    append_localized_update_hunk_with_move(patch, path, old_contents, new_contents, Some(new_path))
}

fn append_hashline_rename_only_hunk(patch: &mut String, path: &str, new_path: &str) {
    patch.push_str("*** Update File: ");
    patch.push_str(path);
    patch.push_str("\n*** Move to: ");
    patch.push_str(new_path);
    patch.push('\n');
}

pub(super) fn build_hashline_patch_preview(
    old_contents: &str,
    new_contents: &str,
) -> Result<HashlinePatchPreview, FunctionCallError> {
    let normalized_old_contents = normalize_file_text(old_contents);
    let normalized_new_contents = normalize_file_text(new_contents);
    let old_lines = split_lines_preserve(&normalized_old_contents);
    let new_lines = split_lines_preserve(&normalized_new_contents);
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

fn split_hashline_operation(input: &str) -> Result<(&str, &str), FunctionCallError> {
    let line = input.trim_start();
    let op_end = line
        .find(|ch: char| ch.is_whitespace() || ch == '|' || ch == ':')
        .unwrap_or(line.len());
    let (op, rest) = line.split_at(op_end);
    if op.is_empty() {
        return Err(invalid_operation_error(line));
    }
    let rest = if rest.starts_with('|') || rest.starts_with(':') {
        rest
    } else {
        rest.trim_start()
    };
    Ok((op, rest))
}

fn invalid_operation_error(line: &str) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "invalid Hashline operation {line}; expected forms like SWAP 12:\n+text, SWAP 12:ab|text, DEL 12, INS.POST 12:\n+text, or INS.TAIL:\n+text"
    ))
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
        if !line_hash_matches(expected_hash, &actual_hash) {
            return Err(FunctionCallError::RespondToModel(format!(
                "line {line_number} hash mismatch: expected {expected_hash}, found {actual_hash}"
            )));
        }
    }
    Ok(())
}

fn line_hash_matches(expected_hash: &str, actual_hash: &str) -> bool {
    let Ok(expected) = u8::from_str_radix(expected_hash, 16) else {
        return false;
    };
    let Ok(actual) = u8::from_str_radix(actual_hash, 16) else {
        return false;
    };
    expected == actual
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
    append_localized_update_hunk_with_move(patch, path, old_contents, new_contents, None)
}

fn append_localized_update_hunk_with_move(
    patch: &mut String,
    path: &str,
    old_contents: &str,
    new_contents: &str,
    new_path: Option<&str>,
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
    if let Some(new_path) = new_path {
        patch.push_str("\n*** Move to: ");
        patch.push_str(new_path);
    }
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
