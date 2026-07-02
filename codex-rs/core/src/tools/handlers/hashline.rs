use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::ApplyPatchHandler;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::hash::Hasher;

const NAMESPACE: &str = "hashline";
const READ_TOOL: &str = "read";
const PATCH_TOOL: &str = "patch";
const FIND_BLOCK_TOOL: &str = "find_block";
const DEFAULT_READ_MAX_LINES: usize = 200;
const HARD_READ_MAX_LINES: usize = 1000;
const DEFAULT_FIND_BLOCK_MAX_LINES: usize = 80;
const HARD_FIND_BLOCK_MAX_LINES: usize = 300;

#[derive(Clone, Copy)]
pub(crate) enum HashlineToolKind {
    Read,
    Patch,
    FindBlock,
}

pub(crate) struct HashlineHandler {
    kind: HashlineToolKind,
    multi_environment: bool,
}

impl HashlineHandler {
    pub(crate) fn new(kind: HashlineToolKind, multi_environment: bool) -> Self {
        Self {
            kind,
            multi_environment,
        }
    }

    fn tool_name_str(&self) -> &'static str {
        match self.kind {
            HashlineToolKind::Read => READ_TOOL,
            HashlineToolKind::Patch => PATCH_TOOL,
            HashlineToolKind::FindBlock => FIND_BLOCK_TOOL,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_lines: Option<usize>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchArgs {
    path: String,
    patch: String,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindBlockArgs {
    path: String,
    anchor: String,
    max_lines: Option<usize>,
    environment_id: Option<String>,
}

impl ToolExecutor<ToolInvocation> for HashlineHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, self.tool_name_str())
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: NAMESPACE.to_string(),
            description: "Tools for hash-anchored file reading and editing. Prefer Hashline for line-anchored edits; broader edit tools may remain available for compatibility."
                .to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(match self.kind {
                HashlineToolKind::Read => read_tool_spec(self.multi_environment),
                HashlineToolKind::Patch => patch_tool_spec(self.multi_environment),
                HashlineToolKind::FindBlock => find_block_tool_spec(self.multi_environment),
            })],
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            match self.kind {
                HashlineToolKind::Read => handle_read(invocation).await,
                HashlineToolKind::Patch => handle_patch(invocation, self.multi_environment).await,
                HashlineToolKind::FindBlock => handle_find_block(invocation).await,
            }
        })
    }
}

impl CoreToolRuntime for HashlineHandler {}

fn read_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: READ_TOOL.to_string(),
        description: "Read a bounded file range with Hashline file and line anchors.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "start_line".to_string(),
                    JsonSchema::integer(Some("1-indexed inclusive start line.".to_string())),
                ),
                (
                    "end_line".to_string(),
                    JsonSchema::integer(Some("1-indexed inclusive end line.".to_string())),
                ),
                (
                    "max_lines".to_string(),
                    JsonSchema::integer(Some(format!(
                        "Maximum lines to return. Defaults to {DEFAULT_READ_MAX_LINES}; hard cap {HARD_READ_MAX_LINES}."
                    ))),
                ),
            ]),
            multi_environment,
            vec!["path".to_string()],
        ),
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "header": { "type": "string" },
                "start_line": { "type": "integer" },
                "end_line": { "type": "integer" },
                "total_lines": { "type": "integer" },
                "truncated": { "type": "boolean" },
                "next_start_line": { "type": ["integer", "null"] },
                "content": { "type": "string" }
            },
            "required": ["path", "header", "start_line", "end_line", "total_lines", "truncated", "next_start_line", "content"],
            "additionalProperties": false
        })),
    }
}

fn patch_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: PATCH_TOOL.to_string(),
        description: "Apply a single-file Hashline line operation patch. Supported operations: SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, and INS.TAIL."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "patch".to_string(),
                    JsonSchema::string(Some(
                        "Hashline operations, one per line. Line anchors use N or N:hh, followed by | and inserted/replacement text when needed."
                            .to_string(),
                    )),
                ),
                (
                    "dry_run".to_string(),
                    JsonSchema::boolean(Some(
                        "Validate the patch and report the resulting file hash without writing."
                            .to_string(),
                    )),
                ),
            ]),
            multi_environment,
            vec!["path".to_string(), "patch".to_string()],
        ),
        output_schema: None,
    }
}

fn find_block_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: FIND_BLOCK_TOOL.to_string(),
        description: "Find a likely syntactic or indentation block around a line anchor and return a bounded anchored excerpt."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "anchor".to_string(),
                    JsonSchema::string(Some(
                        "Line anchor as line or line:hash.".to_string(),
                    )),
                ),
                (
                    "max_lines".to_string(),
                    JsonSchema::integer(Some(format!(
                        "Maximum excerpt lines to return. Defaults to {DEFAULT_FIND_BLOCK_MAX_LINES}; hard cap {HARD_FIND_BLOCK_MAX_LINES}."
                    ))),
                ),
            ]),
            multi_environment,
            vec!["path".to_string(), "anchor".to_string()],
        ),
        output_schema: None,
    }
}

fn schema_with_common_path(
    extra_properties: BTreeMap<String, JsonSchema>,
    multi_environment: bool,
    required: Vec<String>,
) -> JsonSchema {
    let mut properties = BTreeMap::from([(
        "path".to_string(),
        JsonSchema::string(Some(
            "File path resolved relative to the selected environment cwd.".to_string(),
        )),
    )]);
    properties.extend(extra_properties);
    if multi_environment {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Environment id to target when multiple environments are attached.".to_string(),
            )),
        );
    }

    JsonSchema::object(
        properties,
        Some(required),
        /*additional_properties*/ Some(false.into()),
    )
}

async fn handle_read(
    invocation: ToolInvocation,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        turn,
        step_context,
        payload,
        ..
    } = invocation;
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(
            "hashline.read handler received unsupported payload".to_string(),
        ));
    };
    let args: ReadArgs = parse_arguments(&arguments)?;
    let contents = read_selected_file(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let total_lines = count_lines(&contents);
    let max_lines = args
        .max_lines
        .unwrap_or(DEFAULT_READ_MAX_LINES)
        .clamp(1, HARD_READ_MAX_LINES);
    let start_line = args.start_line.unwrap_or(1).max(1);
    let requested_end_line = args.end_line.unwrap_or(usize::MAX).max(start_line);
    let end_line = requested_end_line
        .min(start_line.saturating_add(max_lines).saturating_sub(1))
        .min(total_lines.max(1));
    let truncated = requested_end_line > end_line || end_line < total_lines;
    let next_start_line = truncated.then_some(end_line.saturating_add(1));
    let path_hash = hash_hex(&contents, 4);
    let content = format_hashline_excerpt(&contents, start_line, end_line);
    let body = json!({
        "path": args.path,
        "header": format!("[{}#{}]", args.path, path_hash),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": total_lines,
        "truncated": truncated,
        "next_start_line": next_start_line,
        "content": content,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.read output: {err}")),
        Some(true),
    )))
}

async fn handle_find_block(
    invocation: ToolInvocation,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        turn,
        step_context,
        payload,
        ..
    } = invocation;
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(
            "hashline.find_block handler received unsupported payload".to_string(),
        ));
    };
    let args: FindBlockArgs = parse_arguments(&arguments)?;
    let contents = read_selected_file(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let lines = split_lines_preserve(&contents);
    let anchor_line = parse_anchor_line(&args.anchor)?;
    if anchor_line == 0 || anchor_line > lines.len().max(1) {
        return Err(FunctionCallError::RespondToModel(format!(
            "anchor line {anchor_line} is outside file range 1..={}",
            lines.len()
        )));
    }
    if let Some(expected_hash) = parse_anchor_hash(&args.anchor) {
        let actual_hash = line_hash(lines.get(anchor_line - 1).copied().unwrap_or(""));
        if expected_hash != actual_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "anchor hash mismatch at line {anchor_line}: expected {expected_hash}, found {actual_hash}"
            )));
        }
    }

    let max_lines = args
        .max_lines
        .unwrap_or(DEFAULT_FIND_BLOCK_MAX_LINES)
        .clamp(1, HARD_FIND_BLOCK_MAX_LINES);
    let (block_start, block_end) = find_block_span(&lines, anchor_line);
    let capped_end = block_end.min(block_start.saturating_add(max_lines).saturating_sub(1));
    let body = json!({
        "path": args.path,
        "anchor": args.anchor,
        "start_line": block_start,
        "end_line": block_end,
        "truncated": capped_end < block_end,
        "content": format_hashline_excerpt(&contents, block_start, capped_end),
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.find_block output: {err}")),
        Some(true),
    )))
}

async fn handle_patch(
    invocation: ToolInvocation,
    multi_environment: bool,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        turn,
        step_context,
        payload,
        ..
    } = &invocation;
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch handler received unsupported payload".to_string(),
        ));
    };
    let args: PatchArgs = parse_arguments(arguments)?;
    let contents = read_selected_file(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let patched = apply_hashline_patch(&contents, &args.patch)?;
    let new_hash = hash_hex(&patched, 4);

    if args.dry_run.unwrap_or(false) {
        let body = json!({
            "path": args.path,
            "dry_run": true,
            "old_hash": hash_hex(&contents, 4),
            "new_hash": new_hash,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.patch dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    let apply_patch_text = apply_patch_for_full_file_update(
        &args.path,
        &contents,
        &patched,
        args.environment_id.as_deref(),
    );
    let apply_patch_invocation = ToolInvocation {
        tool_name: ToolName::plain("apply_patch"),
        payload: ToolPayload::Custom {
            input: apply_patch_text,
        },
        ..invocation
    };
    ApplyPatchHandler::new(multi_environment)
        .handle(apply_patch_invocation)
        .await
}

async fn read_selected_file(
    turn: &crate::session::turn_context::TurnContext,
    step_context: &crate::session::step_context::StepContext,
    path: &str,
    environment_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let Some(turn_environment) =
        resolve_tool_environment(&step_context.environments, environment_id)?
    else {
        return Err(FunctionCallError::RespondToModel(
            "hashline file tools are unavailable in this session".to_string(),
        ));
    };
    let path_uri = turn_environment.cwd().join(path).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve path {path} against environment cwd {}: {err}",
            turn_environment.cwd(),
        ))
    })?;
    let sandbox = turn
        .file_system_sandbox_context(/*additional_permissions*/ None, turn_environment.cwd());
    let fs = turn_environment.environment.get_filesystem();
    fs.read_file_text(&path_uri, Some(&sandbox))
        .await
        .map_err(|error| {
            FunctionCallError::RespondToModel(format!("unable to read {path}: {error}"))
        })
}

fn apply_hashline_patch(contents: &str, patch: &str) -> Result<String, FunctionCallError> {
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
        let (op, rest) = line.split_once(' ').ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("invalid Hashline operation {line}"))
        })?;
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

fn parse_insert_text(input: &str) -> Result<&str, FunctionCallError> {
    input.trim_start().strip_prefix('|').ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("insert operation {input} must include |text"))
    })
}

fn parse_anchor_line(anchor: &str) -> Result<usize, FunctionCallError> {
    let line = anchor
        .split_once(':')
        .map_or(anchor, |(line, _)| line)
        .trim();
    line.parse::<usize>().map_err(|err| {
        FunctionCallError::RespondToModel(format!("invalid Hashline anchor {anchor}: {err}"))
    })
}

fn parse_anchor_hash(anchor: &str) -> Option<&str> {
    anchor
        .split_once(':')
        .map(|(_, hash)| hash)
        .filter(|hash| !hash.is_empty())
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

fn apply_patch_for_full_file_update(
    path: &str,
    old_contents: &str,
    new_contents: &str,
    environment_id: Option<&str>,
) -> String {
    let mut patch = String::from("*** Begin Patch\n");
    if let Some(environment_id) = environment_id {
        patch.push_str("*** Environment ID: ");
        patch.push_str(environment_id);
        patch.push('\n');
    }
    patch.push_str("*** Update File: ");
    patch.push_str(path);
    patch.push_str("\n@@\n");
    for line in split_lines_preserve(old_contents) {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in split_lines_preserve(new_contents) {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !old_contents.ends_with('\n') {
        patch.push_str("*** End of File\n");
    }
    patch.push_str("*** End Patch");
    patch
}

fn format_hashline_excerpt(contents: &str, start_line: usize, end_line: usize) -> String {
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

fn split_lines_preserve(contents: &str) -> Vec<&str> {
    let trimmed = contents.strip_suffix('\n').unwrap_or(contents);
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('\n').collect()
    }
}

fn count_lines(contents: &str) -> usize {
    split_lines_preserve(contents).len()
}

fn hash_hex(input: &str, width: usize) -> String {
    let mut hasher = Fnv1a64::default();
    hasher.write(input.as_bytes());
    let mask = if width >= 16 {
        u64::MAX
    } else {
        (1_u64 << (width * 4)) - 1
    };
    format!("{:0width$x}", hasher.finish() & mask)
}

fn line_hash(input: &str) -> String {
    hash_hex(input, 2)
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

fn find_block_span(lines: &[&str], anchor_line: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (1, 1);
    }

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

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| ch.is_whitespace())
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

#[cfg(test)]
#[path = "hashline_tests.rs"]
mod tests;
