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
use std::io;

#[path = "hashline_block.rs"]
mod hashline_block;
#[path = "hashline_format.rs"]
mod hashline_format;
#[path = "hashline_hash.rs"]
mod hashline_hash;
#[path = "hashline_patch.rs"]
mod hashline_patch;

use self::hashline_block::find_block_span;
use self::hashline_block::language_for_path;
use self::hashline_format::count_lines;
use self::hashline_format::format_hashline_excerpt;
use self::hashline_format::split_lines_preserve;
use self::hashline_hash::hash_hex;
use self::hashline_hash::line_hash;
use self::hashline_patch::apply_hashline_patch;
use self::hashline_patch::apply_patch_for_hashline_remove;
use self::hashline_patch::apply_patch_for_hashline_rename;
use self::hashline_patch::apply_patch_for_hashline_update;
use self::hashline_patch::build_hashline_patch_preview;
use self::hashline_patch::ensure_rename_representable;
use self::hashline_patch::parse_anchor_hash;
use self::hashline_patch::parse_anchor_line;
use self::hashline_patch::validate_file_hash;

const PATCH_OUTPUT_MAX_LINES: usize = 40;

const NAMESPACE: &str = "hashline";
const READ_TOOL: &str = "read";
const PATCH_TOOL: &str = "patch";
const FIND_BLOCK_TOOL: &str = "find_block";
const REMOVE_FILE_TOOL: &str = "remove_file";
const RENAME_FILE_TOOL: &str = "rename_file";
const DEFAULT_READ_MAX_LINES: usize = 200;
const HARD_READ_MAX_LINES: usize = 1000;
const DEFAULT_FIND_BLOCK_MAX_LINES: usize = 80;
const HARD_FIND_BLOCK_MAX_LINES: usize = 300;

#[derive(Clone, Copy)]
pub(crate) enum HashlineToolKind {
    Read,
    Patch,
    FindBlock,
    RemoveFile,
    RenameFile,
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
            HashlineToolKind::RemoveFile => REMOVE_FILE_TOOL,
            HashlineToolKind::RenameFile => RENAME_FILE_TOOL,
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
    create: Option<bool>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveFileArgs {
    path: String,
    expected_hash: Option<String>,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameFileArgs {
    path: String,
    new_path: String,
    expected_hash: Option<String>,
    dry_run: Option<bool>,
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
                HashlineToolKind::RemoveFile => remove_file_tool_spec(self.multi_environment),
                HashlineToolKind::RenameFile => rename_file_tool_spec(self.multi_environment),
            })],
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            match self.kind {
                HashlineToolKind::Read => handle_read(invocation).await,
                HashlineToolKind::Patch => handle_patch(invocation, self.multi_environment).await,
                HashlineToolKind::FindBlock => handle_find_block(invocation).await,
                HashlineToolKind::RemoveFile => {
                    handle_remove_file(invocation, self.multi_environment).await
                }
                HashlineToolKind::RenameFile => {
                    handle_rename_file(invocation, self.multi_environment).await
                }
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
        description: "Apply a single-file Hashline line operation patch. Supported operations: SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL, SWAP.BLK, DEL.BLK, and INS.BLK.POST, using either README-style + payload bodies or compact |text forms where supported."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "patch".to_string(),
                    JsonSchema::string(Some(
                        "Hashline operations. Use README-style bodies such as SWAP 12:\n+replacement, SWAP 12..14:\n+replacement, DEL 12..14, INS.POST 12:\n+text, INS.HEAD:\n+text, SWAP.BLK 12:\n+replacement block, DEL.BLK 12, INS.BLK.POST 12:\n+block, or compact forms such as SWAP 12:ab|replacement and INS.TAIL|text."
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
                (
                    "create".to_string(),
                    JsonSchema::boolean(Some(
                        "Create a missing target file. When true, the target must not already exist."
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

fn remove_file_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: REMOVE_FILE_TOOL.to_string(),
        description: "Remove one text file after optional Hashline file-hash validation."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "expected_hash".to_string(),
                    JsonSchema::string(Some(
                        "Optional 4-hex file hash from a Hashline read header.".to_string(),
                    )),
                ),
                (
                    "dry_run".to_string(),
                    JsonSchema::boolean(Some(
                        "Validate the request and report the file hash without deleting."
                            .to_string(),
                    )),
                ),
            ]),
            multi_environment,
            vec!["path".to_string()],
        ),
        output_schema: None,
    }
}

fn rename_file_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: RENAME_FILE_TOOL.to_string(),
        description: "Rename one non-empty newline-terminated text file after optional Hashline file-hash validation."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "new_path".to_string(),
                    JsonSchema::string(Some(
                        "Destination path resolved relative to the selected environment cwd."
                            .to_string(),
                    )),
                ),
                (
                    "expected_hash".to_string(),
                    JsonSchema::string(Some(
                        "Optional 4-hex file hash from a Hashline read header.".to_string(),
                    )),
                ),
                (
                    "dry_run".to_string(),
                    JsonSchema::boolean(Some(
                        "Validate the request and report the file hash without renaming."
                            .to_string(),
                    )),
                ),
            ]),
            multi_environment,
            vec!["path".to_string(), "new_path".to_string()],
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
    let (block_start, block_end) = find_block_span(&args.path, &lines, anchor_line);
    let capped_end = block_end.min(block_start.saturating_add(max_lines).saturating_sub(1));
    let body = json!({
        "path": args.path,
        "anchor": args.anchor,
        "language": language_for_path(&args.path),
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
    let create = args.create.unwrap_or(false);
    let contents = if create {
        ensure_selected_file_missing(
            turn.as_ref(),
            step_context.as_ref(),
            &args.path,
            args.environment_id.as_deref(),
        )
        .await?;
        String::new()
    } else {
        read_selected_file(
            turn.as_ref(),
            step_context.as_ref(),
            &args.path,
            args.environment_id.as_deref(),
        )
        .await?
    };
    let mut patched = apply_hashline_patch(&args.path, &contents, &args.patch)?;
    if create && !patched.is_empty() && !patched.ends_with('\n') {
        patched.push('\n');
    }
    let new_hash = hash_hex(&patched, 4);
    let apply_patch_text = apply_patch_for_hashline_update(
        &args.path,
        &contents,
        &patched,
        create,
        args.environment_id.as_deref(),
    )?;

    if args.dry_run.unwrap_or(false) {
        let preview = build_hashline_patch_preview(&contents, &patched)?;
        let body = json!({
            "path": args.path,
            "dry_run": true,
            "operation": if create { "create" } else { "update" },
            "old_hash": hash_hex(&contents, 4),
            "new_hash": new_hash,
            "preview": preview,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.patch dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    let post_write_turn = std::sync::Arc::clone(turn);
    let post_write_step_context = std::sync::Arc::clone(step_context);
    let apply_patch_invocation = ToolInvocation {
        tool_name: ToolName::plain("apply_patch"),
        payload: ToolPayload::Custom {
            input: apply_patch_text,
        },
        ..invocation
    };
    ApplyPatchHandler::new(multi_environment)
        .handle(apply_patch_invocation)
        .await?;

    let written_contents = read_selected_file(
        post_write_turn.as_ref(),
        post_write_step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let written_hash = hash_hex(&written_contents, 4);
    if written_hash != new_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.patch applied but post-write file hash for {} was {}, expected {new_hash}",
            args.path, written_hash
        )));
    }

    let body = build_hashline_patch_success_body(&args.path, &contents, &written_contents, create)?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.patch output: {err}")),
        Some(true),
    )))
}

fn build_hashline_patch_success_body(
    path: &str,
    old_contents: &str,
    written_contents: &str,
    create: bool,
) -> Result<serde_json::Value, FunctionCallError> {
    let preview = build_hashline_patch_preview(old_contents, written_contents)?;
    let total_lines = count_lines(written_contents);
    let (start_line, end_line, excerpt_truncated) = if total_lines == 0 {
        (None, None, false)
    } else {
        let start_line = preview
            .new_start_line
            .or(preview.old_start_line)
            .unwrap_or(1)
            .min(total_lines)
            .max(1);
        let requested_end_line = preview
            .new_end_line
            .unwrap_or(start_line)
            .clamp(start_line, total_lines);
        let capped_end_line = requested_end_line.min(
            start_line
                .saturating_add(PATCH_OUTPUT_MAX_LINES)
                .saturating_sub(1),
        );
        (
            Some(start_line),
            Some(capped_end_line),
            capped_end_line < requested_end_line,
        )
    };
    let content = start_line
        .zip(end_line)
        .map(|(start_line, end_line)| {
            format_hashline_excerpt(written_contents, start_line, end_line)
        })
        .unwrap_or_default();
    let new_hash = hash_hex(written_contents, 4);

    Ok(json!({
        "success": true,
        "path": path,
        "header": format!("[{path}#{new_hash}]"),
        "operation": if create { "create" } else { "update" },
        "old_hash": hash_hex(old_contents, 4),
        "new_hash": new_hash,
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": total_lines,
        "truncated": preview.truncated || excerpt_truncated,
        "content": content,
        "preview": preview,
    }))
}

async fn handle_remove_file(
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
            "hashline.remove_file handler received unsupported payload".to_string(),
        ));
    };
    let args: RemoveFileArgs = parse_arguments(arguments)?;
    let contents = read_selected_file(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    validate_file_hash(&args.path, &contents, args.expected_hash.as_deref())?;
    let old_hash = hash_hex(&contents, 4);

    if args.dry_run.unwrap_or(false) {
        let body = json!({
            "path": args.path,
            "dry_run": true,
            "old_hash": old_hash,
            "operation": "remove_file",
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.remove_file dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    let apply_patch_text =
        apply_patch_for_hashline_remove(&args.path, args.environment_id.as_deref());
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

async fn handle_rename_file(
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
            "hashline.rename_file handler received unsupported payload".to_string(),
        ));
    };
    let args: RenameFileArgs = parse_arguments(arguments)?;
    let contents = read_selected_file(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    validate_file_hash(&args.path, &contents, args.expected_hash.as_deref())?;
    ensure_rename_representable(&args.path, &contents)?;
    ensure_selected_file_missing(
        turn.as_ref(),
        step_context.as_ref(),
        &args.new_path,
        args.environment_id.as_deref(),
    )
    .await?;
    let old_hash = hash_hex(&contents, 4);

    if args.dry_run.unwrap_or(false) {
        let body = json!({
            "path": args.path,
            "new_path": args.new_path,
            "dry_run": true,
            "old_hash": old_hash,
            "operation": "rename_file",
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.rename_file dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    let apply_patch_text = apply_patch_for_hashline_rename(
        &args.path,
        &args.new_path,
        &contents,
        args.environment_id.as_deref(),
    )?;
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

async fn ensure_selected_file_missing(
    turn: &crate::session::turn_context::TurnContext,
    step_context: &crate::session::step_context::StepContext,
    path: &str,
    environment_id: Option<&str>,
) -> Result<(), FunctionCallError> {
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
    match fs.get_metadata(&path_uri, Some(&sandbox)).await {
        Ok(_) => Err(FunctionCallError::RespondToModel(format!(
            "Hashline operation requires {path} to be missing, but it already exists"
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FunctionCallError::RespondToModel(format!(
            "unable to inspect {path} before create: {error}"
        ))),
    }
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

#[cfg(test)]
#[path = "hashline_tests.rs"]
mod tests;
