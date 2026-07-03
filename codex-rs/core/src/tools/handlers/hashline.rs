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
use self::hashline_hash::normalize_file_text;
use self::hashline_patch::HashlinePatchFileMutation;
use self::hashline_patch::HashlinePatchFileOperation;
use self::hashline_patch::HashlinePatchFileUpdate;
use self::hashline_patch::HashlinePatchPreview;
use self::hashline_patch::HashlinePatchSection;
use self::hashline_patch::apply_hashline_patch;
use self::hashline_patch::apply_patch_for_hashline_mutations;
use self::hashline_patch::apply_patch_for_hashline_remove;
use self::hashline_patch::apply_patch_for_hashline_rename;
use self::hashline_patch::apply_patch_for_hashline_update;
use self::hashline_patch::build_hashline_patch_preview;
use self::hashline_patch::hashline_patch_is_aborted;
use self::hashline_patch::parse_anchor_hash;
use self::hashline_patch::parse_anchor_line;
use self::hashline_patch::parse_hashline_patch_file_operation;
use self::hashline_patch::split_hashline_patch_sections;
use self::hashline_patch::validate_file_hash;
use serde_json::Value;

const PATCH_OUTPUT_MAX_LINES: usize = 40;

const NAMESPACE: &str = "hashline";
const READ_TOOL: &str = "read";
const WRITE_TOOL: &str = "write";
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
    Write,
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
            HashlineToolKind::Write => WRITE_TOOL,
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
    #[serde(alias = "file")]
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_lines: Option<usize>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    #[serde(alias = "file")]
    path: String,
    content: String,
    force: Option<bool>,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchArgs {
    #[serde(alias = "file")]
    path: String,
    patch: String,
    dry_run: Option<bool>,
    create: Option<bool>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindBlockArgs {
    #[serde(alias = "file")]
    path: String,
    anchor: String,
    max_lines: Option<usize>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveFileArgs {
    #[serde(alias = "file")]
    path: String,
    expected_hash: Option<String>,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameFileArgs {
    #[serde(alias = "src")]
    path: String,
    #[serde(alias = "dst")]
    new_path: String,
    expected_hash: Option<String>,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

enum PreparedHashlinePatchFile {
    Update {
        path: String,
        old_contents: String,
        new_contents: String,
        new_hash: String,
        create: bool,
    },
    Remove {
        path: String,
        old_hash: String,
    },
    Rename {
        path: String,
        new_path: String,
        old_contents: String,
        old_hash: String,
    },
}

impl PreparedHashlinePatchFile {
    fn is_update(&self) -> bool {
        matches!(self, PreparedHashlinePatchFile::Update { .. })
    }

    fn is_create(&self) -> bool {
        matches!(self, PreparedHashlinePatchFile::Update { create: true, .. })
    }
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
                HashlineToolKind::Write => write_tool_spec(self.multi_environment),
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
                HashlineToolKind::Write => handle_write(invocation, self.multi_environment).await,
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
                "hash": { "type": "string" },
                "header": { "type": "string" },
                "start_line": { "type": "integer" },
                "end_line": { "type": "integer" },
                "total_lines": { "type": "integer" },
                "truncated": { "type": "boolean" },
                "next_start_line": { "type": ["integer", "null"] },
                "content": { "type": "string" },
                "lines": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "n": { "type": "integer" },
                            "hash": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["n", "hash", "content"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["path", "hash", "header", "start_line", "end_line", "total_lines", "truncated", "next_start_line", "content", "lines"],
            "additionalProperties": false
        })),
    }
}

fn write_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: WRITE_TOOL.to_string(),
        description:
            "Write normalized content, including empty content, to a new file, or overwrite an existing file with force=true."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "content".to_string(),
                    JsonSchema::string(Some(
                        "Complete file content to write. Content is normalized to LF line endings and a leading UTF-8 BOM is stripped. Empty content creates a zero-byte file when the target is missing."
                            .to_string(),
                    )),
                ),
                (
                    "force".to_string(),
                    JsonSchema::boolean(Some(
                        "Overwrite the target if it already exists. Defaults to false.".to_string(),
                    )),
                ),
                (
                    "dry_run".to_string(),
                    JsonSchema::boolean(Some(
                        "Validate and report the resulting file hash without writing.".to_string(),
                    )),
                ),
            ]),
            multi_environment,
            vec!["path".to_string(), "content".to_string()],
        ),
        output_schema: None,
    }
}

fn patch_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: PATCH_TOOL.to_string(),
        description: "Apply a Hashline line operation patch. Supports one target file by default, multiple existing files when split into [path#HASH] sections, or multiple missing files with create=true and [path] sections. Supported operations: SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL, SWAP.BLK, DEL.BLK, INS.BLK.POST, INS.BLK.PRE, INS.BLK, and sectioned REM/MV file ops, using either README-style + payload bodies or compact |text forms where supported."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "patch".to_string(),
                    JsonSchema::string(Some(
                        "Hashline operations. Use README-style bodies such as SWAP 12:\n+replacement, SWAP 12:ab..14:cd:\n+replacement, DEL 12..=14, DEL 12-14, INS.POST 12:\n+text, INS.HEAD:\n+text, SWAP.BLK 12:\n+replacement block, DEL.BLK 12, INS.BLK.POST 12:\n+block, INS.BLK.PRE 12:\n+block, INS.BLK 12:\n+block, compact forms such as SWAP 12:ab|replacement and INS.TAIL|text, [path#HASH] sections for existing-file multi-file edits, or [path] sections with create=true for missing files. Bracketed headers also recover common apply-patch-style path noise such as [*** Update File: path#HASH]. Bare payload lines are accepted after an operation header, and uniformly pasted read-output rows such as 1:ab|content or >>> 1:ab|content have their read prefixes stripped. Empty create sections create zero-byte files. In payload bodies, use ++ for literal + and +- for literal -. Sectioned patches also accept REM, MV <path>, and *** Abort to suppress an embedded patch."
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
                        "Create missing target files. When true, every target must not already exist; empty patches create zero-byte files; use [path] sections for multi-file creation."
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
                        "Line anchor as line, line:hash, block N:, or a unique short line hash.".to_string(),
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
        description: "Rename one text file after optional Hashline file-hash validation."
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
    let body = build_hashline_read_body(
        &args.path,
        &contents,
        args.start_line.unwrap_or(1),
        args.end_line,
        args.max_lines.unwrap_or(DEFAULT_READ_MAX_LINES),
    );
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.read output: {err}")),
        Some(true),
    )))
}

fn build_hashline_read_body(
    path: &str,
    contents: &str,
    start_line: usize,
    requested_end_line: Option<usize>,
    max_lines: usize,
) -> Value {
    let total_lines = count_lines(contents);
    let max_lines = max_lines.clamp(1, HARD_READ_MAX_LINES);
    let start_line = start_line.max(1);
    let requested_end_line = requested_end_line
        .unwrap_or_else(|| total_lines.max(start_line))
        .max(start_line);
    let end_line = requested_end_line
        .min(start_line.saturating_add(max_lines).saturating_sub(1))
        .min(total_lines.max(1));
    let truncated = requested_end_line > end_line || end_line < total_lines;
    let next_start_line = truncated.then_some(end_line.saturating_add(1));
    let path_hash = hash_hex(contents, 4);
    let content = format_hashline_excerpt(contents, start_line, end_line);
    let lines = build_hashline_line_rows(contents, start_line, end_line);
    json!({
        "path": path,
        "hash": path_hash,
        "header": format!("[{path}#{path_hash}]"),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": total_lines,
        "truncated": truncated,
        "next_start_line": next_start_line,
        "content": content,
        "lines": lines,
    })
}

fn build_hashline_line_rows(contents: &str, start_line: usize, end_line: usize) -> Vec<Value> {
    if start_line > end_line {
        return Vec::new();
    }
    let normalized = normalize_file_text(contents);
    split_lines_preserve(&normalized)
        .into_iter()
        .enumerate()
        .skip(start_line.saturating_sub(1))
        .take(end_line.saturating_sub(start_line).saturating_add(1))
        .map(|(index, line)| {
            json!({
                "n": index + 1,
                "hash": line_hash(line),
                "content": line,
            })
        })
        .collect()
}

async fn handle_write(
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
            "hashline.write handler received unsupported payload".to_string(),
        ));
    };
    let args: WriteArgs = parse_arguments(arguments)?;
    let create = !selected_file_exists(
        turn.as_ref(),
        step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    if !create && !args.force.unwrap_or(false) {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.write refuses to overwrite existing file {}; set force=true to overwrite",
            args.path
        )));
    }

    let old_contents = if create {
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
    let mut write_contents = normalize_file_text(&args.content);
    if create && !write_contents.is_empty() && !write_contents.ends_with('\n') {
        write_contents.push('\n');
    }
    let operation = if create { "create" } else { "update" };
    let new_hash = hash_hex(&write_contents, 4);
    if args.dry_run.unwrap_or(false) {
        let preview = (old_contents != write_contents)
            .then(|| build_hashline_patch_preview(&old_contents, &write_contents))
            .transpose()?;
        let body = json!({
            "path": args.path,
            "dry_run": true,
            "operation": operation,
            "old_hash": hash_hex(&old_contents, 4),
            "new_hash": new_hash,
            "preview": preview,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.write dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    if create || old_contents != write_contents {
        let apply_patch_text = apply_patch_for_hashline_update(
            &args.path,
            &old_contents,
            &write_contents,
            create,
            args.environment_id.as_deref(),
        )?;
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

        let written_contents = read_selected_file_after_update(
            post_write_turn.as_ref(),
            post_write_step_context.as_ref(),
            &args.path,
            args.environment_id.as_deref(),
            &write_contents,
            &new_hash,
            "hashline.write",
        )
        .await?;
        return build_hashline_write_output(
            &args.path,
            &old_contents,
            &written_contents,
            operation,
        );
    }

    build_hashline_write_output(&args.path, &old_contents, &write_contents, operation)
}

fn build_hashline_write_output(
    path: &str,
    old_contents: &str,
    written_contents: &str,
    operation: &str,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let mut body = build_hashline_read_body(
        path,
        written_contents,
        /*start_line*/ 1,
        None,
        DEFAULT_READ_MAX_LINES,
    );
    let new_hash = hash_hex(written_contents, 4);
    let Some(body_object) = body.as_object_mut() else {
        return Err(FunctionCallError::RespondToModel(
            "failed to construct hashline.write output".to_string(),
        ));
    };
    body_object.insert("success".to_string(), Value::Bool(true));
    body_object.insert(
        "operation".to_string(),
        Value::String(operation.to_string()),
    );
    body_object.insert(
        "old_hash".to_string(),
        Value::String(hash_hex(old_contents, 4)),
    );
    body_object.insert("new_hash".to_string(), Value::String(new_hash));
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.write output: {err}")),
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
    let normalized_contents = normalize_file_text(&contents);
    let lines = split_lines_preserve(&normalized_contents);
    let anchor_line = resolve_find_block_anchor(&args.anchor, &lines)?;

    let max_lines = args
        .max_lines
        .unwrap_or(DEFAULT_FIND_BLOCK_MAX_LINES)
        .clamp(1, HARD_FIND_BLOCK_MAX_LINES);
    let (block_start, block_end) = find_block_span(&args.path, &lines, anchor_line);
    let capped_end = block_end.min(block_start.saturating_add(max_lines).saturating_sub(1));
    let block_lines = lines
        .iter()
        .enumerate()
        .skip(block_start.saturating_sub(1))
        .take(capped_end.saturating_sub(block_start).saturating_add(1))
        .map(|(index, line)| {
            json!({
                "n": index + 1,
                "hash": line_hash(line),
                "content": line,
            })
        })
        .collect::<Vec<_>>();
    let body = json!({
        "file": &args.path,
        "path": &args.path,
        "anchor": args.anchor,
        "line_count": lines.len(),
        "language": language_for_path(&args.path),
        "start_line": block_start,
        "end_line": block_end,
        "truncated": capped_end < block_end,
        "content": format_hashline_excerpt(&normalized_contents, block_start, capped_end),
        "block_lines": block_lines,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.find_block output: {err}")),
        Some(true),
    )))
}

fn resolve_find_block_anchor(anchor: &str, lines: &[&str]) -> Result<usize, FunctionCallError> {
    let trimmed = anchor.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(line_text) = lower
        .strip_prefix("block ")
        .and_then(|rest| rest.strip_suffix(':'))
    {
        let anchor_line = parse_anchor_line(line_text)?;
        validate_find_block_line(anchor_line, lines)?;
        return Ok(anchor_line);
    }

    if trimmed.parse::<usize>().is_err() && is_short_hash(trimmed) {
        return resolve_unique_line_hash(trimmed, lines);
    }

    let anchor_line = parse_anchor_line(trimmed)?;
    validate_find_block_line(anchor_line, lines)?;
    if let Some(expected_hash) = parse_anchor_hash(trimmed) {
        let actual_hash = line_hash(lines.get(anchor_line - 1).copied().unwrap_or(""));
        if expected_hash.to_ascii_lowercase() != actual_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "anchor hash mismatch at line {anchor_line}: expected {expected_hash}, found {actual_hash}"
            )));
        }
    }
    Ok(anchor_line)
}

fn validate_find_block_line(anchor_line: usize, lines: &[&str]) -> Result<(), FunctionCallError> {
    if anchor_line == 0 || anchor_line > lines.len().max(1) {
        return Err(FunctionCallError::RespondToModel(format!(
            "anchor line {anchor_line} is outside file range 1..={}",
            lines.len()
        )));
    }
    Ok(())
}

fn resolve_unique_line_hash(hash: &str, lines: &[&str]) -> Result<usize, FunctionCallError> {
    let hash = hash.to_ascii_lowercase();
    let matching_lines = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line_hash(line) == hash).then_some(index + 1))
        .collect::<Vec<_>>();
    match matching_lines.as_slice() {
        [] => Err(FunctionCallError::RespondToModel(format!(
            "anchor hash {hash} was not found"
        ))),
        [line] => Ok(*line),
        matches => Err(FunctionCallError::RespondToModel(format!(
            "anchor hash {hash} is ambiguous; matching lines: {}",
            matches
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn is_short_hash(value: &str) -> bool {
    value.len() == 2 && value.chars().all(|ch| ch.is_ascii_hexdigit())
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
    if hashline_patch_is_aborted(&args.patch) {
        return Ok(build_hashline_patch_aborted_output(
            &args.path,
            args.dry_run.unwrap_or(false),
        ));
    }
    let create = args.create.unwrap_or(false);
    let sections = split_hashline_patch_sections(&args.path, &args.patch)?;
    if sections.len() > 1 {
        return handle_multi_file_patch(&invocation, &args, &sections, multi_environment).await;
    }

    let Some(section) = sections.first() else {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch did not contain any file sections".to_string(),
        ));
    };
    let target_path = &section.path;
    if create && section.expected_hash.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "hashline.patch create=true cannot use a [path#HASH] section header because the target file must be missing"
                .to_string(),
        ));
    }

    let contents = if create {
        ensure_selected_file_missing(
            turn.as_ref(),
            step_context.as_ref(),
            target_path,
            args.environment_id.as_deref(),
        )
        .await?;
        String::new()
    } else {
        read_selected_file(
            turn.as_ref(),
            step_context.as_ref(),
            target_path,
            args.environment_id.as_deref(),
        )
        .await?
    };
    validate_file_hash(target_path, &contents, section.expected_hash.as_deref())?;

    if let Some(file_operation) = parse_hashline_patch_file_operation(&section.patch)? {
        if create {
            return Err(FunctionCallError::RespondToModel(
                "hashline.patch create=true cannot be combined with REM or MV".to_string(),
            ));
        }
        return handle_patch_file_operation(
            &invocation,
            multi_environment,
            target_path,
            &contents,
            file_operation,
            args.dry_run.unwrap_or(false),
            args.environment_id.as_deref(),
        )
        .await;
    }

    let mut patched =
        apply_hashline_patch_or_create_empty(target_path, &contents, &section.patch, create)?;
    if create && !patched.is_empty() && !patched.ends_with('\n') {
        patched.push('\n');
    }
    let new_hash = hash_hex(&patched, 4);
    let apply_patch_text = apply_patch_for_hashline_update(
        target_path,
        &contents,
        &patched,
        create,
        args.environment_id.as_deref(),
    )?;

    if args.dry_run.unwrap_or(false) {
        let preview = build_hashline_patch_preview_or_none(&contents, &patched, create)?;
        let body = json!({
            "path": target_path,
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

    let written_contents = read_selected_file_after_update(
        post_write_turn.as_ref(),
        post_write_step_context.as_ref(),
        target_path,
        args.environment_id.as_deref(),
        &patched,
        &new_hash,
        "hashline.patch",
    )
    .await?;

    let body =
        build_hashline_patch_success_body(target_path, &contents, &written_contents, create)?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.patch output: {err}")),
        Some(true),
    )))
}

fn build_hashline_patch_aborted_output(
    path: &str,
    dry_run: bool,
) -> Box<dyn codex_tools::ToolOutput> {
    let body = json!({
        "success": true,
        "path": path,
        "operation": "abort",
        "aborted": true,
        "dry_run": dry_run,
    });
    boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
            format!("failed to serialize hashline.patch abort output: {err}")
        }),
        Some(true),
    ))
}

async fn handle_patch_file_operation(
    invocation: &ToolInvocation,
    multi_environment: bool,
    path: &str,
    contents: &str,
    file_operation: HashlinePatchFileOperation,
    dry_run: bool,
    environment_id: Option<&str>,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let old_hash = hash_hex(contents, 4);
    match file_operation {
        HashlinePatchFileOperation::Remove => {
            if dry_run {
                let body = json!({
                    "path": path,
                    "dry_run": true,
                    "old_hash": old_hash,
                    "operation": "remove_file",
                });
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                        format!("failed to serialize hashline.patch remove dry-run output: {err}")
                    }),
                    Some(true),
                )));
            }

            let apply_patch_text = apply_patch_for_hashline_remove(path, environment_id);
            let post_remove_turn = std::sync::Arc::clone(&invocation.turn);
            let post_remove_step_context = std::sync::Arc::clone(&invocation.step_context);
            let apply_patch_invocation = ToolInvocation {
                tool_name: ToolName::plain("apply_patch"),
                payload: ToolPayload::Custom {
                    input: apply_patch_text,
                },
                ..invocation.clone()
            };
            ApplyPatchHandler::new(multi_environment)
                .handle(apply_patch_invocation)
                .await?;
            ensure_selected_file_missing(
                post_remove_turn.as_ref(),
                post_remove_step_context.as_ref(),
                path,
                environment_id,
            )
            .await?;
            let body = json!({
                "success": true,
                "path": path,
                "operation": "remove_file",
                "old_hash": old_hash,
            });
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                    format!("failed to serialize hashline.patch remove output: {err}")
                }),
                Some(true),
            )))
        }
        HashlinePatchFileOperation::Rename { new_path } => {
            ensure_selected_file_missing(
                invocation.turn.as_ref(),
                invocation.step_context.as_ref(),
                &new_path,
                environment_id,
            )
            .await?;
            if dry_run {
                let body = json!({
                    "path": path,
                    "new_path": new_path,
                    "src": path,
                    "dst": &new_path,
                    "dry_run": true,
                    "old_hash": old_hash,
                    "operation": "rename_file",
                });
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                        format!("failed to serialize hashline.patch rename dry-run output: {err}")
                    }),
                    Some(true),
                )));
            }

            let apply_patch_text =
                apply_patch_for_hashline_rename(path, &new_path, contents, environment_id);
            let post_rename_turn = std::sync::Arc::clone(&invocation.turn);
            let post_rename_step_context = std::sync::Arc::clone(&invocation.step_context);
            let apply_patch_invocation = ToolInvocation {
                tool_name: ToolName::plain("apply_patch"),
                payload: ToolPayload::Custom {
                    input: apply_patch_text,
                },
                ..invocation.clone()
            };
            ApplyPatchHandler::new(multi_environment)
                .handle(apply_patch_invocation)
                .await?;
            ensure_selected_file_missing(
                post_rename_turn.as_ref(),
                post_rename_step_context.as_ref(),
                path,
                environment_id,
            )
            .await?;
            let renamed_contents = read_selected_file(
                post_rename_turn.as_ref(),
                post_rename_step_context.as_ref(),
                &new_path,
                environment_id,
            )
            .await?;
            let new_hash = hash_hex(&renamed_contents, 4);
            if new_hash != old_hash {
                return Err(FunctionCallError::RespondToModel(format!(
                    "hashline.patch rename completed but destination hash for {new_path} was {new_hash}, expected {old_hash}"
                )));
            }
            let body = json!({
                "success": true,
                "path": path,
                "new_path": new_path,
                "src": path,
                "dst": new_path,
                "operation": "rename_file",
                "old_hash": old_hash,
                "new_hash": new_hash,
                "header": format!("[{new_path}#{new_hash}]"),
            });
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                    format!("failed to serialize hashline.patch rename output: {err}")
                }),
                Some(true),
            )))
        }
    }
}

async fn handle_multi_file_patch(
    invocation: &ToolInvocation,
    args: &PatchArgs,
    sections: &[HashlinePatchSection],
    multi_environment: bool,
) -> Result<Box<dyn codex_tools::ToolOutput>, FunctionCallError> {
    let mut prepared_files = Vec::new();
    let create = args.create.unwrap_or(false);
    for section in sections {
        if create && section.expected_hash.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "hashline.patch create=true cannot use [path#HASH] section headers because every target file must be missing"
                    .to_string(),
            ));
        }
        let old_contents = if create {
            ensure_selected_file_missing(
                invocation.turn.as_ref(),
                invocation.step_context.as_ref(),
                &section.path,
                args.environment_id.as_deref(),
            )
            .await?;
            String::new()
        } else {
            read_selected_file(
                invocation.turn.as_ref(),
                invocation.step_context.as_ref(),
                &section.path,
                args.environment_id.as_deref(),
            )
            .await?
        };
        validate_file_hash(
            &section.path,
            &old_contents,
            section.expected_hash.as_deref(),
        )?;

        if let Some(file_operation) = parse_hashline_patch_file_operation(&section.patch)? {
            if create {
                return Err(FunctionCallError::RespondToModel(
                    "hashline.patch create=true cannot be combined with REM or MV".to_string(),
                ));
            }
            match file_operation {
                HashlinePatchFileOperation::Remove => {
                    let old_hash = hash_hex(&old_contents, 4);
                    prepared_files.push(PreparedHashlinePatchFile::Remove {
                        path: section.path.clone(),
                        old_hash,
                    });
                }
                HashlinePatchFileOperation::Rename { new_path } => {
                    ensure_selected_file_missing(
                        invocation.turn.as_ref(),
                        invocation.step_context.as_ref(),
                        &new_path,
                        args.environment_id.as_deref(),
                    )
                    .await?;
                    let old_hash = hash_hex(&old_contents, 4);
                    prepared_files.push(PreparedHashlinePatchFile::Rename {
                        path: section.path.clone(),
                        new_path,
                        old_contents,
                        old_hash,
                    });
                }
            }
            continue;
        }

        let mut new_contents = apply_hashline_patch_or_create_empty(
            &section.path,
            &old_contents,
            &section.patch,
            create,
        )?;
        if create && !new_contents.is_empty() && !new_contents.ends_with('\n') {
            new_contents.push('\n');
        }
        let new_hash = hash_hex(&new_contents, 4);
        prepared_files.push(PreparedHashlinePatchFile::Update {
            path: section.path.clone(),
            old_contents,
            new_contents,
            new_hash,
            create,
        });
    }

    let operation = if prepared_files
        .iter()
        .all(PreparedHashlinePatchFile::is_create)
    {
        "multi_file_create"
    } else if prepared_files
        .iter()
        .all(PreparedHashlinePatchFile::is_update)
    {
        "multi_file_update"
    } else {
        "multi_file_operation"
    };

    if args.dry_run.unwrap_or(false) {
        let files = prepared_files
            .iter()
            .map(|file| match file {
                PreparedHashlinePatchFile::Update {
                    path,
                    old_contents,
                    new_contents,
                    new_hash,
                    create,
                } => {
                    let preview =
                        build_hashline_patch_preview_or_none(old_contents, new_contents, *create)?;
                    Ok(json!({
                        "path": path,
                        "operation": if *create { "create" } else { "update" },
                        "old_hash": hash_hex(old_contents, 4),
                        "new_hash": new_hash,
                        "preview": preview,
                    }))
                }
                PreparedHashlinePatchFile::Remove { path, old_hash, .. } => Ok(json!({
                    "path": path,
                    "operation": "remove_file",
                    "old_hash": old_hash,
                })),
                PreparedHashlinePatchFile::Rename {
                    path,
                    new_path,
                    old_hash,
                    ..
                } => Ok(json!({
                    "path": path,
                    "new_path": new_path,
                    "src": path,
                    "dst": new_path,
                    "operation": "rename_file",
                    "old_hash": old_hash,
                })),
            })
            .collect::<Result<Vec<_>, FunctionCallError>>()?;
        let body = json!({
            "dry_run": true,
            "operation": operation,
            "files": files,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
                format!("failed to serialize hashline.patch multi-file dry-run output: {err}")
            }),
            Some(true),
        )));
    }

    let mutations = prepared_files
        .iter()
        .map(|file| match file {
            PreparedHashlinePatchFile::Update {
                path,
                old_contents,
                new_contents,
                create,
                ..
            } => HashlinePatchFileMutation::Update(HashlinePatchFileUpdate {
                path,
                old_contents,
                new_contents,
                create: *create,
            }),
            PreparedHashlinePatchFile::Remove { path, .. } => {
                HashlinePatchFileMutation::Remove { path }
            }
            PreparedHashlinePatchFile::Rename {
                path,
                new_path,
                old_contents,
                ..
            } => HashlinePatchFileMutation::Rename {
                path,
                new_path,
                contents: old_contents,
            },
        })
        .collect::<Vec<_>>();
    let apply_patch_text =
        apply_patch_for_hashline_mutations(&mutations, args.environment_id.as_deref())?;
    let apply_patch_invocation = ToolInvocation {
        tool_name: ToolName::plain("apply_patch"),
        payload: ToolPayload::Custom {
            input: apply_patch_text,
        },
        ..invocation.clone()
    };
    ApplyPatchHandler::new(multi_environment)
        .handle(apply_patch_invocation)
        .await?;

    let mut files = Vec::new();
    for file in &prepared_files {
        match file {
            PreparedHashlinePatchFile::Update {
                path,
                old_contents,
                new_contents,
                new_hash,
                create,
            } => {
                let written_contents = read_selected_file_after_update(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    path,
                    args.environment_id.as_deref(),
                    new_contents,
                    new_hash,
                    "hashline.patch",
                )
                .await?;
                files.push(build_hashline_patch_success_body(
                    path,
                    old_contents,
                    &written_contents,
                    *create,
                )?);
            }
            PreparedHashlinePatchFile::Remove { path, old_hash, .. } => {
                ensure_selected_file_missing(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    path,
                    args.environment_id.as_deref(),
                )
                .await?;
                files.push(json!({
                    "success": true,
                    "path": path,
                    "operation": "remove_file",
                    "old_hash": old_hash,
                }));
            }
            PreparedHashlinePatchFile::Rename {
                path,
                new_path,
                old_hash,
                ..
            } => {
                ensure_selected_file_missing(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    path,
                    args.environment_id.as_deref(),
                )
                .await?;
                let renamed_contents = read_selected_file(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    new_path,
                    args.environment_id.as_deref(),
                )
                .await?;
                let new_hash = hash_hex(&renamed_contents, 4);
                if new_hash != *old_hash {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "hashline.patch rename completed but destination hash for {new_path} was {new_hash}, expected {old_hash}"
                    )));
                }
                files.push(json!({
                    "success": true,
                    "path": path,
                    "new_path": new_path,
                    "src": path,
                    "dst": new_path,
                    "operation": "rename_file",
                    "old_hash": old_hash,
                    "new_hash": new_hash,
                    "header": format!("[{new_path}#{new_hash}]"),
                }));
            }
        }
    }

    let body = json!({
        "success": true,
        "operation": operation,
        "files": files,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
            format!("failed to serialize hashline.patch multi-file output: {err}")
        }),
        Some(true),
    )))
}

fn build_hashline_patch_success_body(
    path: &str,
    old_contents: &str,
    written_contents: &str,
    create: bool,
) -> Result<serde_json::Value, FunctionCallError> {
    let preview = build_hashline_patch_preview_or_none(old_contents, written_contents, create)?;
    let total_lines = count_lines(written_contents);
    let (start_line, end_line, excerpt_truncated) = if total_lines == 0 {
        (None, None, false)
    } else {
        let start_line = preview
            .as_ref()
            .and_then(|preview| preview.new_start_line.or(preview.old_start_line))
            .unwrap_or(1)
            .min(total_lines)
            .max(1);
        let requested_end_line = preview
            .as_ref()
            .and_then(|preview| preview.new_end_line)
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
    let lines = start_line
        .zip(end_line)
        .map(|(start_line, end_line)| {
            build_hashline_line_rows(written_contents, start_line, end_line)
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
        "truncated": preview.as_ref().is_some_and(|preview| preview.truncated) || excerpt_truncated,
        "content": content,
        "lines": lines,
        "preview": preview,
    }))
}

fn apply_hashline_patch_or_create_empty(
    target_path: &str,
    contents: &str,
    patch: &str,
    create: bool,
) -> Result<String, FunctionCallError> {
    if create && patch.trim().is_empty() {
        Ok(String::new())
    } else {
        apply_hashline_patch(target_path, contents, patch)
    }
}

fn build_hashline_patch_preview_or_none(
    old_contents: &str,
    new_contents: &str,
    create: bool,
) -> Result<Option<HashlinePatchPreview>, FunctionCallError> {
    if create && old_contents == new_contents {
        Ok(None)
    } else {
        build_hashline_patch_preview(old_contents, new_contents).map(Some)
    }
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
    let post_remove_turn = std::sync::Arc::clone(turn);
    let post_remove_step_context = std::sync::Arc::clone(step_context);
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

    ensure_selected_file_missing(
        post_remove_turn.as_ref(),
        post_remove_step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let body = json!({
        "success": true,
        "path": args.path,
        "operation": "remove_file",
        "old_hash": old_hash,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
            format!("failed to serialize hashline.remove_file output: {err}")
        }),
        Some(true),
    )))
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
            "path": &args.path,
            "new_path": &args.new_path,
            "src": &args.path,
            "dst": &args.new_path,
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
    );
    let post_rename_turn = std::sync::Arc::clone(turn);
    let post_rename_step_context = std::sync::Arc::clone(step_context);
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

    ensure_selected_file_missing(
        post_rename_turn.as_ref(),
        post_rename_step_context.as_ref(),
        &args.path,
        args.environment_id.as_deref(),
    )
    .await?;
    let renamed_contents = read_selected_file(
        post_rename_turn.as_ref(),
        post_rename_step_context.as_ref(),
        &args.new_path,
        args.environment_id.as_deref(),
    )
    .await?;
    let new_hash = hash_hex(&renamed_contents, 4);
    if new_hash != old_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.rename_file completed but destination hash for {} was {}, expected {old_hash}",
            args.new_path, new_hash
        )));
    }
    let header = format!("[{}#{}]", args.new_path, new_hash);
    let body = json!({
        "success": true,
        "path": &args.path,
        "new_path": &args.new_path,
        "src": &args.path,
        "dst": &args.new_path,
        "operation": "rename_file",
        "old_hash": old_hash,
        "new_hash": new_hash,
        "header": header,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string_pretty(&body).unwrap_or_else(|err| {
            format!("failed to serialize hashline.rename_file output: {err}")
        }),
        Some(true),
    )))
}

async fn ensure_selected_file_missing(
    turn: &crate::session::turn_context::TurnContext,
    step_context: &crate::session::step_context::StepContext,
    path: &str,
    environment_id: Option<&str>,
) -> Result<(), FunctionCallError> {
    if selected_file_exists(turn, step_context, path, environment_id).await? {
        return Err(FunctionCallError::RespondToModel(format!(
            "Hashline operation requires {path} to be missing, but it already exists"
        )));
    }
    Ok(())
}

async fn selected_file_exists(
    turn: &crate::session::turn_context::TurnContext,
    step_context: &crate::session::step_context::StepContext,
    path: &str,
    environment_id: Option<&str>,
) -> Result<bool, FunctionCallError> {
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
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(FunctionCallError::RespondToModel(format!(
            "unable to inspect {path}: {error}"
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

async fn read_selected_file_after_update(
    turn: &crate::session::turn_context::TurnContext,
    step_context: &crate::session::step_context::StepContext,
    path: &str,
    environment_id: Option<&str>,
    expected_contents: &str,
    expected_hash: &str,
    operation: &str,
) -> Result<String, FunctionCallError> {
    let written_contents = read_selected_file(turn, step_context, path, environment_id).await?;
    let written_hash = hash_hex(&written_contents, 4);
    if written_hash != expected_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "{operation} applied but post-write file hash for {path} was {written_hash}, expected {expected_hash}"
        )));
    }
    if written_contents == expected_contents {
        return Ok(written_contents);
    }

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
    fs.write_file(
        &path_uri,
        expected_contents.as_bytes().to_vec(),
        Some(&sandbox),
    )
    .await
    .map_err(|error| {
        FunctionCallError::RespondToModel(format!(
            "{operation} could not restore exact contents for {path}: {error}"
        ))
    })?;

    let rewritten_contents = read_selected_file(turn, step_context, path, environment_id).await?;
    if rewritten_contents != expected_contents {
        return Err(FunctionCallError::RespondToModel(format!(
            "{operation} restored {path} but exact contents still differ from the Hashline result"
        )));
    }
    Ok(rewritten_contents)
}

#[cfg(test)]
#[path = "hashline_tests.rs"]
mod tests;
