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

use self::hashline_block::find_normalized_block_span;
use self::hashline_block::language_for_path;
use self::hashline_format::build_hashline_excerpt;
use self::hashline_format::split_lines_preserve;
use self::hashline_hash::FILE_HASH_WIDTH;
use self::hashline_hash::LINE_HASH_WIDTH;
use self::hashline_hash::hash_hex;
use self::hashline_hash::hash_normalized_hex;
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
use self::hashline_patch::hashline_patch_has_line_operations;
use self::hashline_patch::hashline_patch_is_aborted;
use self::hashline_patch::hashline_patch_warnings;
use self::hashline_patch::parse_anchor_hash;
use self::hashline_patch::parse_anchor_line;
use self::hashline_patch::parse_hashline_patch_file_operation;
use self::hashline_patch::split_hashline_patch_sections;
use self::hashline_patch::split_hashline_patch_sections_for_create;
use self::hashline_patch::validate_file_hash;
use serde_json::Value;

const PATCH_OUTPUT_MAX_LINES: usize = 40;
const READ_EXCERPT_MAX_SERIALIZED_BYTES: usize = 24 * 1024;
const FIND_BLOCK_EXCERPT_MAX_SERIALIZED_BYTES: usize = 24 * 1024;
const PATCH_EXCERPT_MAX_SERIALIZED_BYTES: usize = 4 * 1024;
const MULTI_FILE_DETAILS_MAX_SERIALIZED_BYTES: usize = 24 * 1024;

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
    expected_hash: String,
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
    expected_hash: String,
    dry_run: Option<bool>,
    environment_id: Option<String>,
}

enum PreparedHashlinePatchFile {
    Update {
        path: String,
        old_contents: String,
        new_contents: String,
        new_hash: String,
        warnings: Vec<String>,
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
        new_contents: String,
        new_hash: String,
        warnings: Vec<String>,
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
            description: "Tools for hash-anchored file reading and editing. Prefer Hashline for line-anchored edits; broader edit tools may remain available."
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
                        "Maximum lines to return. Defaults to {DEFAULT_READ_MAX_LINES}; hard cap {HARD_READ_MAX_LINES}. Output may stop earlier at the serialized byte budget."
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
                "start_line": { "type": ["integer", "null"] },
                "end_line": { "type": ["integer", "null"] },
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
                            "content_truncated": { "type": "boolean" }
                        },
                        "required": ["n", "hash"],
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
        description: "Apply a Hashline line operation patch. Supports one target file by default, multiple existing files when split into [path]#HASH sections, or multiple missing files with create=true and [path] sections. Supported operations: SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL, SWAP.BLK, DEL.BLK, INS.BLK.POST, INS.BLK.PRE, INS.BLK, sectioned REM file ops, and MV file ops that may also include line edits, using README-style + payload bodies or compact |text forms where supported."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "patch".to_string(),
                    JsonSchema::string(Some(
                        "Hashline operations. Guardrails: start operation lines only with documented operation tokens; copy line and file hash anchors verbatim from a recent hashline.read or find_block output and never invent or reconstruct hashes; prefix each payload body line with + so it cannot be parsed as patch structure; after a file or line hash mismatch, reread the file and rebuild the patch from the refreshed anchors before retrying. Use README-style bodies such as SWAP 12:1a2b:\n+replacement, SWAP 12:1a2b..14:3c4d:\n+replacement, DEL 12:1a2b..=14:3c4d, INS.POST 12:1a2b:\n+text, INS.HEAD:\n+text, SWAP.BLK 12:1a2b@0123abcd:\n+replacement block, DEL.BLK 12:1a2b@0123abcd, INS.BLK.POST 12:1a2b@0123abcd:\n+block, INS.BLK.PRE 12:1a2b@0123abcd:\n+block, INS.BLK 12:1a2b@0123abcd:\n+block, compact forms such as SWAP 12:1a2b|replacement and INS.TAIL|text, [path]#HASH sections for existing-file multi-file edits, or [path] sections with create=true for missing files. Bracketed headers also recover common apply-patch-style path noise such as [*** Update File: path]#HASH. Bare payload lines are accepted after an operation header, and uniformly pasted bare read-output rows such as 1:1a2b|content or >>> 1:1a2b|content have their read prefixes stripped. Plus-prefixed payload rows are explicit literals after removing the patch + marker, so +1:1a2b|content writes 1:1a2b|content. Empty create sections create zero-byte files. In payload bodies, use ++ for literal + and +- for literal -. Sectioned patches also accept REM and MV <path>; MV may be combined with line operations to rename and edit one section, while REM must stand alone. *** Abort suppresses an embedded patch."
                            .to_string(),
                    )),
                ),
                (
                    "dry_run".to_string(),
                    JsonSchema::boolean(Some(
                        "Validate the patch and report the resulting file hash without writing. Changed-line previews and multi-file details are byte-bounded."
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
                        "Anchor forms: line:4-hex, block N:, a unique 4-hex line hash, or line:4-hex@8-hex-block-hash; bare line numbers are not accepted."
                            .to_string(),
                    )),
                ),
                (
                    "max_lines".to_string(),
                    JsonSchema::integer(Some(format!(
                        "Maximum excerpt lines to return. Defaults to {DEFAULT_FIND_BLOCK_MAX_LINES}; hard cap {HARD_FIND_BLOCK_MAX_LINES}. Output may stop earlier at the serialized byte budget."
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
        description: "Remove one text file after required Hashline file-hash validation."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: schema_with_common_path(
            BTreeMap::from([
                (
                    "expected_hash".to_string(),
                    JsonSchema::string(Some(
                        "Required 8-hex file hash from a Hashline read header.".to_string(),
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
            vec!["path".to_string(), "expected_hash".to_string()],
        ),
        output_schema: None,
    }
}

fn rename_file_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    ResponsesApiTool {
        name: RENAME_FILE_TOOL.to_string(),
        description: "Rename one text file after required Hashline file-hash validation."
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
                        "Required 8-hex file hash from a Hashline read header.".to_string(),
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
            vec![
                "path".to_string(),
                "new_path".to_string(),
                "expected_hash".to_string(),
            ],
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
    )?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string(&body)
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
) -> Result<Value, FunctionCallError> {
    if start_line == 0 {
        return Err(FunctionCallError::RespondToModel(
            "hashline.read start_line must be at least 1".to_string(),
        ));
    }
    if max_lines == 0 {
        return Err(FunctionCallError::RespondToModel(
            "hashline.read max_lines must be at least 1".to_string(),
        ));
    }
    if requested_end_line.is_some_and(|end_line| end_line < start_line) {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.read end_line must be greater than or equal to start_line {start_line}"
        )));
    }

    let normalized = normalize_file_text(contents);
    let all_lines = split_lines_preserve(&normalized);
    let total_lines = all_lines.len();
    if start_line > total_lines.max(1) {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.read start_line {start_line} is outside file range 1..={total_lines}"
        )));
    }
    let path_hash = hash_normalized_hex(&normalized);
    let (start_line, end_line, truncated, next_start_line, content, lines) = if total_lines == 0 {
        (None, None, false, None, String::new(), Vec::new())
    } else {
        let max_lines = max_lines.clamp(1, HARD_READ_MAX_LINES);
        let requested_end_line = requested_end_line.unwrap_or(total_lines).min(total_lines);
        let capped_end_line = requested_end_line
            .min(start_line.saturating_add(max_lines).saturating_sub(1))
            .min(total_lines);
        let excerpt = build_hashline_excerpt(
            &all_lines,
            start_line,
            capped_end_line,
            READ_EXCERPT_MAX_SERIALIZED_BYTES,
        );
        let end_line = excerpt.end_line.unwrap_or(start_line);
        let truncated = capped_end_line < requested_end_line || excerpt.truncated;
        let next_start_line =
            (truncated && end_line < total_lines).then_some(end_line.saturating_add(1));
        (
            Some(start_line),
            Some(end_line),
            truncated,
            next_start_line,
            excerpt.content,
            excerpt.lines,
        )
    };
    Ok(json!({
        "path": path,
        "hash": path_hash,
        "header": format!("[{path}]#{path_hash}"),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": total_lines,
        "truncated": truncated,
        "next_start_line": next_start_line,
        "content": content,
        "lines": lines,
    }))
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
    let write_contents = normalize_file_text(&args.content).into_owned();
    let operation = if create { "create" } else { "update" };
    let new_hash = hash_hex(&write_contents);
    if args.dry_run.unwrap_or(false) {
        let preview = (old_contents != write_contents)
            .then(|| build_hashline_patch_preview(&old_contents, &write_contents))
            .transpose()?;
        let body = json!({
            "success": true,
            "path": args.path,
            "dry_run": true,
            "operation": operation,
            "old_hash": hash_hex(&old_contents),
            "new_hash": new_hash,
            "preview": preview,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string(&body).unwrap_or_else(|err| {
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
    let body = build_hashline_patch_success_body(
        path,
        old_contents,
        written_contents,
        operation == "create",
    )?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string(&body)
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
    let anchor_line = resolve_find_block_anchor(&args.path, &args.anchor, &lines)?;

    let max_lines = args
        .max_lines
        .unwrap_or(DEFAULT_FIND_BLOCK_MAX_LINES)
        .clamp(1, HARD_FIND_BLOCK_MAX_LINES);
    let (block_start, block_end) = find_normalized_block_span(&args.path, &lines, anchor_line);
    let capped_end = block_end.min(block_start.saturating_add(max_lines).saturating_sub(1));
    let excerpt = build_hashline_excerpt(
        &lines,
        block_start,
        capped_end,
        FIND_BLOCK_EXCERPT_MAX_SERIALIZED_BYTES,
    );
    let file_hash = hash_hex(&normalized_contents);
    let block_hash = hash_hex(&lines[block_start - 1..block_end].join("\n"));
    let anchor_hash = line_hash(lines[anchor_line - 1]);
    let block_anchor = format!("{anchor_line}:{anchor_hash}@{block_hash}");
    let file_header = format!(
        "[{path}]#{file_hash}",
        path = args.path.as_str(),
        file_hash = &file_hash
    );
    let body = json!({
        "file": &args.path,
        "path": &args.path,
        "hash": file_hash,
        "header": file_header,
        "block_hash": block_hash,
        "block_anchor": block_anchor,
        "anchor": args.anchor,
        "line_count": lines.len(),
        "language": language_for_path(&args.path),
        "start_line": block_start,
        "end_line": block_end,
        "truncated": capped_end < block_end || excerpt.truncated,
        "content": excerpt.content,
        "block_lines": excerpt.lines,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string(&body)
            .unwrap_or_else(|err| format!("failed to serialize hashline.find_block output: {err}")),
        Some(true),
    )))
}

fn resolve_find_block_anchor(
    path: &str,
    anchor: &str,
    lines: &[&str],
) -> Result<usize, FunctionCallError> {
    let trimmed = anchor.trim();
    if let Some((line_anchor, block_hash)) = trimmed.rsplit_once('@') {
        let line_anchor = line_anchor.trim();
        let block_hash = block_hash.trim().to_ascii_lowercase();
        let anchor_line = parse_anchor_line(line_anchor)?;
        validate_find_block_line(anchor_line, lines)?;
        let Some(expected_line_hash) = parse_anchor_hash(line_anchor) else {
            return Err(FunctionCallError::RespondToModel(format!(
                "invalid Hashline block anchor {anchor}: expected line:4-hex@{FILE_HASH_WIDTH}-hex"
            )));
        };
        if !is_line_hash(expected_line_hash) {
            return Err(FunctionCallError::RespondToModel(format!(
                "invalid Hashline block anchor {anchor}: expected a {LINE_HASH_WIDTH}-hex line hash"
            )));
        }
        let actual_line_hash = line_hash(lines[anchor_line - 1]);
        if expected_line_hash.to_ascii_lowercase() != actual_line_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "anchor hash mismatch at line {anchor_line}: expected {expected_line_hash}, found {actual_line_hash}"
            )));
        }
        if block_hash.len() != FILE_HASH_WIDTH
            || !block_hash.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(FunctionCallError::RespondToModel(format!(
                "invalid Hashline block anchor {anchor}: expected a {FILE_HASH_WIDTH}-hex block hash"
            )));
        }
        let (block_start, block_end) = find_normalized_block_span(path, lines, anchor_line);
        let actual_block_hash = hash_hex(&lines[block_start - 1..block_end].join("\n"));
        if actual_block_hash != block_hash {
            return Err(FunctionCallError::RespondToModel(format!(
                "block hash mismatch: expected {block_hash}, found {actual_block_hash}"
            )));
        }
        return Ok(anchor_line);
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some(line_text) = lower
        .strip_prefix("block ")
        .and_then(|rest| rest.strip_suffix(':'))
    {
        let anchor_line = parse_anchor_line(line_text)?;
        validate_find_block_line(anchor_line, lines)?;
        return Ok(anchor_line);
    }

    if is_line_hash(trimmed) {
        return resolve_unique_line_hash(trimmed, lines);
    }
    if trimmed.parse::<usize>().is_ok() {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline block anchor {anchor}: bare line numbers are not accepted; use line:4-hex, block N:, a unique 4-hex line hash, or line:4-hex@8-hex-block-hash"
        )));
    }
    let anchor_line = parse_anchor_line(trimmed)?;
    validate_find_block_line(anchor_line, lines)?;
    let Some(expected_hash) = parse_anchor_hash(trimmed) else {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline anchor {anchor}: expected a {LINE_HASH_WIDTH}-hex hash"
        )));
    };
    if !is_line_hash(expected_hash) {
        return Err(FunctionCallError::RespondToModel(format!(
            "invalid Hashline anchor {anchor}: expected a {LINE_HASH_WIDTH}-hex hash"
        )));
    }
    let actual_hash = line_hash(lines[anchor_line - 1]);
    if expected_hash.to_ascii_lowercase() != actual_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "anchor hash mismatch at line {anchor_line}: expected {expected_hash}, found {actual_hash}"
        )));
    }
    Ok(anchor_line)
}

fn validate_find_block_line(anchor_line: usize, lines: &[&str]) -> Result<(), FunctionCallError> {
    if anchor_line == 0 || anchor_line > lines.len() {
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

fn is_line_hash(value: &str) -> bool {
    value.len() == LINE_HASH_WIDTH && value.chars().all(|ch| ch.is_ascii_hexdigit())
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
    let sections = if create {
        split_hashline_patch_sections_for_create(&args.path, &args.patch)?
    } else {
        split_hashline_patch_sections(&args.path, &args.patch)?
    };
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
            "hashline.patch create=true cannot use a [path]#HASH section header because the target file must be missing"
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
    if !create {
        let Some(expected_hash) = section.expected_hash.as_deref() else {
            return Err(FunctionCallError::RespondToModel(format!(
                "existing-file Hashline patches require a [{target_path}]#HASH section header"
            )));
        };
        validate_file_hash(target_path, &contents, expected_hash)?;
    }

    if let Some(file_operation) = parse_hashline_patch_file_operation(&section.patch)? {
        if create {
            return Err(FunctionCallError::RespondToModel(
                "hashline.patch create=true cannot be combined with REM or MV".to_string(),
            ));
        }
        match file_operation {
            HashlinePatchFileOperation::Remove => {
                return handle_patch_file_operation(
                    &invocation,
                    multi_environment,
                    target_path,
                    &contents,
                    HashlinePatchFileOperation::Remove,
                    args.dry_run.unwrap_or(false),
                    args.environment_id.as_deref(),
                )
                .await;
            }
            HashlinePatchFileOperation::Rename { new_path } => {
                if !hashline_patch_has_line_operations(&section.patch)? {
                    return handle_patch_file_operation(
                        &invocation,
                        multi_environment,
                        target_path,
                        &contents,
                        HashlinePatchFileOperation::Rename { new_path },
                        args.dry_run.unwrap_or(false),
                        args.environment_id.as_deref(),
                    )
                    .await;
                }

                ensure_selected_file_missing(
                    turn.as_ref(),
                    step_context.as_ref(),
                    &new_path,
                    args.environment_id.as_deref(),
                )
                .await?;
                let patched = apply_hashline_patch_or_create_empty(
                    target_path,
                    &contents,
                    &section.patch,
                    /*create*/ false,
                )?;
                let warnings = hashline_patch_warnings(&section.patch)?;
                let new_hash = hash_hex(&patched);
                let apply_patch_text = apply_patch_for_hashline_mutations(
                    &[HashlinePatchFileMutation::Rename {
                        path: target_path,
                        new_path: &new_path,
                        old_contents: &contents,
                        new_contents: &patched,
                    }],
                    args.environment_id.as_deref(),
                )?;

                if args.dry_run.unwrap_or(false) {
                    let preview = if contents == patched {
                        None
                    } else {
                        build_hashline_patch_preview_or_none(
                            &contents, &patched, /*create*/ false,
                        )?
                    };
                    let mut body = json!({
                        "success": true,
                        "path": target_path,
                        "new_path": &new_path,
                        "src": target_path,
                        "dst": &new_path,
                        "dry_run": true,
                        "old_hash": hash_hex(&contents),
                        "new_hash": new_hash,
                        "operation": "rename_file",
                        "preview": preview,
                    });
                    add_hashline_warnings(&mut body, &warnings);
                    return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                        serde_json::to_string(&body).unwrap_or_else(|err| {
                            format!(
                                "failed to serialize hashline.patch rename dry-run output: {err}"
                            )
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

                ensure_selected_file_missing(
                    post_write_turn.as_ref(),
                    post_write_step_context.as_ref(),
                    target_path,
                    args.environment_id.as_deref(),
                )
                .await?;
                let written_contents = read_selected_file_after_update(
                    post_write_turn.as_ref(),
                    post_write_step_context.as_ref(),
                    &new_path,
                    args.environment_id.as_deref(),
                    &patched,
                    &new_hash,
                    "hashline.patch rename",
                )
                .await?;
                let mut body = build_hashline_rename_update_success_body(
                    target_path,
                    &new_path,
                    &contents,
                    &written_contents,
                )?;
                add_hashline_warnings(&mut body, &warnings);
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    serde_json::to_string(&body).unwrap_or_else(|err| {
                        format!("failed to serialize hashline.patch rename output: {err}")
                    }),
                    Some(true),
                )));
            }
        }
    }

    let patched =
        apply_hashline_patch_or_create_empty(target_path, &contents, &section.patch, create)?;
    let warnings = hashline_patch_warnings(&section.patch)?;
    let new_hash = hash_hex(&patched);
    let apply_patch_text = apply_patch_for_hashline_update(
        target_path,
        &contents,
        &patched,
        create,
        args.environment_id.as_deref(),
    )?;

    if args.dry_run.unwrap_or(false) {
        let preview = build_hashline_patch_preview_or_none(&contents, &patched, create)?;
        let mut body = json!({
            "success": true,
            "path": target_path,
            "dry_run": true,
            "operation": if create { "create" } else { "update" },
            "old_hash": hash_hex(&contents),
            "new_hash": new_hash,
            "preview": preview,
        });
        add_hashline_warnings(&mut body, &warnings);
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string(&body).unwrap_or_else(|err| {
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

    let mut body =
        build_hashline_patch_success_body(target_path, &contents, &written_contents, create)?;
    add_hashline_warnings(&mut body, &warnings);
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string(&body)
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
        serde_json::to_string(&body).unwrap_or_else(|err| {
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
    let old_hash = hash_hex(contents);
    match file_operation {
        HashlinePatchFileOperation::Remove => {
            if dry_run {
                let body = json!({
                    "success": true,
                    "path": path,
                    "dry_run": true,
                    "old_hash": old_hash,
                    "operation": "remove_file",
                });
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    serde_json::to_string(&body).unwrap_or_else(|err| {
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
                serde_json::to_string(&body).unwrap_or_else(|err| {
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
                    "success": true,
                    "path": path,
                    "new_path": new_path,
                    "src": path,
                    "dst": &new_path,
                    "dry_run": true,
                    "old_hash": old_hash,
                    "operation": "rename_file",
                });
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    serde_json::to_string(&body).unwrap_or_else(|err| {
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
            let new_hash = hash_hex(&renamed_contents);
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
                "header": format!("[{new_path}]#{new_hash}"),
            });
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::to_string(&body).unwrap_or_else(|err| {
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
                "hashline.patch create=true cannot use [path]#HASH section headers because every target file must be missing"
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
        if !create {
            let Some(expected_hash) = section.expected_hash.as_deref() else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "existing-file Hashline patches require a [{}]#HASH section header",
                    section.path
                )));
            };
            validate_file_hash(&section.path, &old_contents, expected_hash)?;
        }

        if let Some(file_operation) = parse_hashline_patch_file_operation(&section.patch)? {
            if create {
                return Err(FunctionCallError::RespondToModel(
                    "hashline.patch create=true cannot be combined with REM or MV".to_string(),
                ));
            }
            match file_operation {
                HashlinePatchFileOperation::Remove => {
                    let old_hash = hash_hex(&old_contents);
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
                    let old_hash = hash_hex(&old_contents);
                    let has_line_operations = hashline_patch_has_line_operations(&section.patch)?;
                    let (new_contents, new_hash, warnings) = if has_line_operations {
                        let new_contents = apply_hashline_patch_or_create_empty(
                            &section.path,
                            &old_contents,
                            &section.patch,
                            /*create*/ false,
                        )?;
                        let new_hash = hash_hex(&new_contents);
                        let warnings = hashline_patch_warnings(&section.patch)?;
                        (new_contents, new_hash, warnings)
                    } else {
                        (old_contents.clone(), old_hash.clone(), Vec::new())
                    };
                    prepared_files.push(PreparedHashlinePatchFile::Rename {
                        path: section.path.clone(),
                        new_path,
                        old_contents,
                        old_hash,
                        new_contents,
                        new_hash,
                        warnings,
                    });
                }
            }
            continue;
        }

        let new_contents = apply_hashline_patch_or_create_empty(
            &section.path,
            &old_contents,
            &section.patch,
            create,
        )?;
        let new_hash = hash_hex(&new_contents);
        let warnings = hashline_patch_warnings(&section.patch)?;
        prepared_files.push(PreparedHashlinePatchFile::Update {
            path: section.path.clone(),
            old_contents,
            new_contents,
            new_hash,
            warnings,
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
        let total_files = prepared_files.len();
        let mut files = Vec::new();
        let mut serialized_bytes = 0;
        for file in &prepared_files {
            let detail = match file {
                PreparedHashlinePatchFile::Update {
                    path,
                    old_contents,
                    new_contents,
                    new_hash,
                    warnings,
                    create,
                } => {
                    let preview =
                        build_hashline_patch_preview_or_none(old_contents, new_contents, *create)?;
                    let mut body = json!({
                        "success": true,
                        "path": path,
                        "operation": if *create { "create" } else { "update" },
                        "old_hash": hash_hex(old_contents),
                        "new_hash": new_hash,
                        "preview": preview,
                    });
                    add_hashline_warnings(&mut body, warnings);
                    body
                }
                PreparedHashlinePatchFile::Remove { path, old_hash, .. } => json!({
                    "success": true,
                    "path": path,
                    "operation": "remove_file",
                    "old_hash": old_hash,
                }),
                PreparedHashlinePatchFile::Rename {
                    path,
                    new_path,
                    old_contents,
                    old_hash,
                    new_contents,
                    new_hash,
                    warnings,
                    ..
                } => {
                    let preview = if old_contents == new_contents {
                        None
                    } else {
                        build_hashline_patch_preview_or_none(
                            old_contents,
                            new_contents,
                            /*create*/ false,
                        )?
                    };
                    let mut body = json!({
                        "success": true,
                        "path": path,
                        "new_path": new_path,
                        "src": path,
                        "dst": new_path,
                        "operation": "rename_file",
                        "old_hash": old_hash,
                        "new_hash": new_hash,
                        "preview": preview,
                    });
                    add_hashline_warnings(&mut body, warnings);
                    body
                }
            };
            if !push_bounded_file_detail(&mut files, &mut serialized_bytes, detail) {
                break;
            }
        }
        let body = json!({
            "success": true,
            "dry_run": true,
            "operation": operation,
            "total_files": total_files,
            "files_truncated": files.len() < total_files,
            "files": files,
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string(&body).unwrap_or_else(|err| {
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
                new_contents,
                ..
            } => HashlinePatchFileMutation::Rename {
                path,
                new_path,
                old_contents,
                new_contents,
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

    let total_files = prepared_files.len();
    let mut files = Vec::new();
    let mut serialized_bytes = 0;
    let mut details_full = false;
    for file in &prepared_files {
        let detail = match file {
            PreparedHashlinePatchFile::Update {
                path,
                old_contents,
                new_contents,
                new_hash,
                warnings,
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
                if details_full {
                    None
                } else {
                    let mut body = build_hashline_patch_success_body(
                        path,
                        old_contents,
                        &written_contents,
                        *create,
                    )?;
                    add_hashline_warnings(&mut body, warnings);
                    Some(body)
                }
            }
            PreparedHashlinePatchFile::Remove { path, old_hash, .. } => {
                ensure_selected_file_missing(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    path,
                    args.environment_id.as_deref(),
                )
                .await?;
                (!details_full).then(|| {
                    json!({
                        "success": true,
                        "path": path,
                        "operation": "remove_file",
                        "old_hash": old_hash,
                    })
                })
            }
            PreparedHashlinePatchFile::Rename {
                path,
                new_path,
                old_contents,
                old_hash,
                new_hash,
                warnings,
                new_contents,
            } => {
                ensure_selected_file_missing(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    path,
                    args.environment_id.as_deref(),
                )
                .await?;
                let renamed_contents = read_selected_file_after_update(
                    invocation.turn.as_ref(),
                    invocation.step_context.as_ref(),
                    new_path,
                    args.environment_id.as_deref(),
                    new_contents,
                    new_hash,
                    "hashline.patch rename",
                )
                .await?;
                if details_full {
                    None
                } else {
                    let mut body = if old_hash == new_hash {
                        json!({
                            "success": true,
                            "path": path,
                            "new_path": new_path,
                            "src": path,
                            "dst": new_path,
                            "operation": "rename_file",
                            "old_hash": old_hash,
                            "new_hash": new_hash,
                            "header": format!("[{new_path}]#{new_hash}"),
                        })
                    } else {
                        build_hashline_rename_update_success_body(
                            path,
                            new_path,
                            old_contents,
                            &renamed_contents,
                        )?
                    };
                    add_hashline_warnings(&mut body, warnings);
                    Some(body)
                }
            }
        };
        if let Some(detail) = detail
            && !push_bounded_file_detail(&mut files, &mut serialized_bytes, detail)
        {
            details_full = true;
        }
    }

    let body = json!({
        "success": true,
        "operation": operation,
        "total_files": total_files,
        "files_truncated": files.len() < total_files,
        "files": files,
    });
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        serde_json::to_string(&body).unwrap_or_else(|err| {
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
    let normalized = normalize_file_text(written_contents);
    let all_lines = split_lines_preserve(&normalized);
    let total_lines = all_lines.len();
    let (start_line, end_line, excerpt_truncated, content, lines) = if total_lines == 0 {
        (None, None, false, String::new(), Vec::new())
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
        let excerpt = build_hashline_excerpt(
            &all_lines,
            start_line,
            capped_end_line,
            PATCH_EXCERPT_MAX_SERIALIZED_BYTES,
        );
        let end_line = excerpt.end_line.unwrap_or(start_line);
        (
            Some(start_line),
            Some(end_line),
            capped_end_line < requested_end_line || excerpt.truncated,
            excerpt.content,
            excerpt.lines,
        )
    };
    let new_hash = hash_normalized_hex(&normalized);

    Ok(json!({
        "success": true,
        "path": path,
        "header": format!("[{path}]#{new_hash}"),
        "operation": if create { "create" } else { "update" },
        "old_hash": hash_hex(old_contents),
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

fn build_hashline_rename_update_success_body(
    path: &str,
    new_path: &str,
    old_contents: &str,
    written_contents: &str,
) -> Result<serde_json::Value, FunctionCallError> {
    let mut body = build_hashline_patch_success_body(
        new_path,
        old_contents,
        written_contents,
        /*create*/ false,
    )?;
    if let Some(body_object) = body.as_object_mut() {
        body_object.insert("path".to_string(), json!(path));
        body_object.insert("new_path".to_string(), json!(new_path));
        body_object.insert("src".to_string(), json!(path));
        body_object.insert("dst".to_string(), json!(new_path));
        body_object.insert("operation".to_string(), json!("rename_file"));
    }
    Ok(body)
}

fn add_hashline_warnings(body: &mut Value, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    if let Some(body_object) = body.as_object_mut() {
        body_object.insert("warnings".to_string(), json!(warnings));
    }
}

fn push_bounded_file_detail(
    files: &mut Vec<Value>,
    serialized_bytes: &mut usize,
    detail: Value,
) -> bool {
    let detail_bytes = serde_json::to_string(&detail).map_or(usize::MAX, |value| {
        value
            .len()
            .saturating_add(value.lines().count().saturating_mul(4))
    });
    let next_bytes = serialized_bytes
        .saturating_add(detail_bytes)
        .saturating_add(1);
    if next_bytes > MULTI_FILE_DETAILS_MAX_SERIALIZED_BYTES {
        return false;
    }
    *serialized_bytes = next_bytes;
    files.push(detail);
    true
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
    validate_file_hash(&args.path, &contents, &args.expected_hash)?;
    let old_hash = hash_hex(&contents);

    if args.dry_run.unwrap_or(false) {
        let body = json!({
            "success": true,
            "path": args.path,
            "dry_run": true,
            "old_hash": old_hash,
            "operation": "remove_file",
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string(&body).unwrap_or_else(|err| {
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
        serde_json::to_string(&body).unwrap_or_else(|err| {
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
    validate_file_hash(&args.path, &contents, &args.expected_hash)?;
    ensure_selected_file_missing(
        turn.as_ref(),
        step_context.as_ref(),
        &args.new_path,
        args.environment_id.as_deref(),
    )
    .await?;
    let old_hash = hash_hex(&contents);

    if args.dry_run.unwrap_or(false) {
        let body = json!({
            "success": true,
            "path": &args.path,
            "new_path": &args.new_path,
            "src": &args.path,
            "dst": &args.new_path,
            "dry_run": true,
            "old_hash": old_hash,
            "operation": "rename_file",
        });
        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::to_string(&body).unwrap_or_else(|err| {
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
    let new_hash = hash_hex(&renamed_contents);
    if new_hash != old_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "hashline.rename_file completed but destination hash for {} was {}, expected {old_hash}",
            args.new_path, new_hash
        )));
    }
    let header = format!("[{}]#{}", args.new_path, new_hash);
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
        serde_json::to_string(&body).unwrap_or_else(|err| {
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
    let written_hash = hash_hex(&written_contents);
    let normalized_expected_contents = normalize_file_text(expected_contents);
    let normalized_written_contents = normalize_file_text(&written_contents);
    let apply_patch_added_trailing_newline = !normalized_expected_contents.ends_with('\n')
        && normalized_written_contents.strip_suffix('\n')
            == Some(normalized_expected_contents.as_ref());
    if written_hash != expected_hash && !apply_patch_added_trailing_newline {
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
            "{operation} could not restore exact contents for {path} (post-write hash {written_hash}, expected {expected_hash}): {error}"
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
