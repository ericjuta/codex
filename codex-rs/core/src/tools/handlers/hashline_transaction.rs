use codex_exec_server::HashlineTransactionExecuteAction;
use codex_exec_server::HashlineTransactionExecuteRequest;
use codex_exec_server::HashlineTransactionExecuteResponse;
use codex_exec_server::HashlineTransactionPlanRequest;
use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::ExpectedFile;
use codex_hashline_transaction::FileEdit;
use codex_hashline_transaction::FileMutation;
use codex_hashline_transaction::LineAnchor;
use codex_hashline_transaction::LineRange;
use codex_hashline_transaction::MutationPreview;
use codex_hashline_transaction::PlanPreview;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::hashline_transaction_spec::transaction_tool_spec;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const NAMESPACE: &str = "hashline";
pub(super) const TOOL_NAME: &str = "transaction";
const MAX_MODEL_TRANSACTION_BYTES: usize = 8 * 1024;

fn serialize_model_preview(
    preview: &PlanPreview,
    output_name: &'static str,
    serialize: &mut impl FnMut(&PlanPreview) -> Result<String, serde_json::Error>,
) -> Result<String, FunctionCallError> {
    serialize(preview).map_err(|error| {
        FunctionCallError::RespondToModel(format!(
            "failed to serialize hashline.transaction {output_name}: {error}"
        ))
    })
}

fn bounded_preview_serialization(
    preview: &mut PlanPreview,
    output_name: &'static str,
    mut serialize: impl FnMut(&PlanPreview) -> Result<String, serde_json::Error>,
) -> Result<String, FunctionCallError> {
    let mut output = serialize_model_preview(preview, output_name, &mut serialize)?;
    if output.len() > MAX_MODEL_TRANSACTION_BYTES {
        preview.preview_truncated = true;
        for index in (0..preview.mutations.len()).rev() {
            {
                let content = match &mut preview.mutations[index] {
                    MutationPreview::Create { content, .. }
                    | MutationPreview::Update { content, .. }
                    | MutationPreview::Delete { content, .. }
                    | MutationPreview::Move { content, .. } => content,
                };
                preview.preview_bytes = preview
                    .preview_bytes
                    .saturating_sub(content.text.len() as u64);
                if !content.text.is_empty() {
                    content.text.clear();
                    content.truncated = true;
                }
            }
            output = serialize_model_preview(preview, output_name, &mut serialize)?;
            if output.len() <= MAX_MODEL_TRANSACTION_BYTES {
                break;
            }
        }
        while output.len() > MAX_MODEL_TRANSACTION_BYTES && preview.mutations.pop().is_some() {
            output = serialize_model_preview(preview, output_name, &mut serialize)?;
        }
    }
    if output.len() > MAX_MODEL_TRANSACTION_BYTES {
        Err(FunctionCallError::RespondToModel(format!(
            "hashline.transaction {output_name} metadata exceeds the {MAX_MODEL_TRANSACTION_BYTES}-byte model output limit"
        )))
    } else {
        Ok(output)
    }
}

fn bounded_execution_output(
    response: HashlineTransactionExecuteResponse,
) -> Result<String, FunctionCallError> {
    let HashlineTransactionExecuteResponse {
        mut preview,
        transaction_id,
        outcome,
    } = response;
    bounded_preview_serialization(&mut preview, "commit result", |preview| {
        serde_json::to_string(&serde_json::json!({
            "preview": preview,
            "transactionId": &transaction_id,
            "outcome": &outcome,
        }))
    })
}

pub(crate) struct HashlineTransactionHandler {
    multi_environment: bool,
}

impl HashlineTransactionHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum ToolTransactionAction {
    Preview,
    Commit,
    CommitPreviewed {
        #[serde(rename = "expectedPlanDigest", alias = "expected_plan_digest")]
        expected_plan_digest: ExactBytesDigest,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolExpectedFile {
    exact_digest: ExactBytesDigest,
}

impl From<ToolExpectedFile> for ExpectedFile {
    fn from(expected: ToolExpectedFile) -> Self {
        Self {
            exact_digest: expected.exact_digest,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolLineAnchor {
    line: u64,
    expected_hash: String,
}

impl From<ToolLineAnchor> for LineAnchor {
    fn from(anchor: ToolLineAnchor) -> Self {
        Self {
            line: anchor.line,
            expected_hash: anchor.expected_hash,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolLineRange {
    start: ToolLineAnchor,
    end: ToolLineAnchor,
}

impl From<ToolLineRange> for LineRange {
    fn from(range: ToolLineRange) -> Self {
        Self {
            start: range.start.into(),
            end: range.end.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum ToolFileEdit {
    ReplaceAll {
        contents: String,
    },
    ReplaceLines {
        range: ToolLineRange,
        lines: Vec<String>,
    },
    InsertBefore {
        anchor: ToolLineAnchor,
        lines: Vec<String>,
    },
    InsertAfter {
        anchor: ToolLineAnchor,
        lines: Vec<String>,
    },
}

impl From<ToolFileEdit> for FileEdit {
    fn from(edit: ToolFileEdit) -> Self {
        match edit {
            ToolFileEdit::ReplaceAll { contents } => Self::ReplaceAll {
                contents: contents.into_bytes(),
            },
            ToolFileEdit::ReplaceLines { range, lines } => Self::ReplaceLines {
                range: range.into(),
                lines,
            },
            ToolFileEdit::InsertBefore { anchor, lines } => Self::InsertBefore {
                anchor: anchor.into(),
                lines,
            },
            ToolFileEdit::InsertAfter { anchor, lines } => Self::InsertAfter {
                anchor: anchor.into(),
                lines,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum ToolFileMutation {
    Create {
        path: String,
        contents: String,
    },
    Update {
        path: String,
        expected: ToolExpectedFile,
        edits: Vec<ToolFileEdit>,
    },
    Delete {
        path: String,
        expected: ToolExpectedFile,
    },
    Move {
        source: String,
        expected: ToolExpectedFile,
        destination: String,
        edits: Vec<ToolFileEdit>,
    },
}

impl From<ToolFileMutation> for FileMutation {
    fn from(mutation: ToolFileMutation) -> Self {
        match mutation {
            ToolFileMutation::Create { path, contents } => Self::Create {
                path,
                contents: contents.into_bytes(),
            },
            ToolFileMutation::Update {
                path,
                expected,
                edits,
            } => Self::Update {
                path,
                expected: expected.into(),
                edits: edits.into_iter().map(Into::into).collect(),
            },
            ToolFileMutation::Delete { path, expected } => Self::Delete {
                path,
                expected: expected.into(),
            },
            ToolFileMutation::Move {
                source,
                expected,
                destination,
                edits,
            } => Self::Move {
                source,
                expected: expected.into(),
                destination,
                edits: edits.into_iter().map(Into::into).collect(),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HashlineTransactionArgs {
    action: ToolTransactionAction,
    root: Option<String>,
    mutations: Vec<ToolFileMutation>,
    #[serde(rename = "environment_id")]
    environment_id: Option<String>,
}

impl ToolExecutor<ToolInvocation> for HashlineTransactionHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: NAMESPACE.to_string(),
            description: "Tools for hash-anchored file reading and editing. Prefer Hashline for line-anchored edits; broader edit tools may remain available."
                .to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(transaction_tool_spec(
                self.multi_environment,
            ))],
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(handle_transaction(invocation))
    }
}

impl CoreToolRuntime for HashlineTransactionHandler {}

async fn handle_transaction(
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
            "hashline.transaction handler received unsupported payload".to_string(),
        ));
    };
    let args: HashlineTransactionArgs = parse_arguments(&arguments)?;
    let Some(turn_environment) =
        resolve_tool_environment(&step_context.environments, args.environment_id.as_deref())?
    else {
        return Err(FunctionCallError::RespondToModel(
            "hashline.transaction is unavailable in this session".to_string(),
        ));
    };
    let root = match args.root {
        Some(root) => turn_environment.cwd().join(&root).map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "unable to resolve transaction root {root} against environment cwd {}: {error}",
                turn_environment.cwd()
            ))
        })?,
        None => turn_environment.cwd().clone(),
    };
    let sandbox =
        turn.file_system_sandbox_context(/*additional_permissions*/ None, turn_environment);
    let mutations = args.mutations.into_iter().map(Into::into).collect();
    let execute_action = match args.action {
        ToolTransactionAction::Preview => None,
        ToolTransactionAction::Commit => Some(HashlineTransactionExecuteAction::Commit),
        ToolTransactionAction::CommitPreviewed {
            expected_plan_digest,
        } => Some(HashlineTransactionExecuteAction::CommitPreviewed {
            expected_plan_digest,
        }),
    };
    let output = if let Some(action) = execute_action {
        let response = turn_environment
            .environment
            .execute_hashline_transaction(HashlineTransactionExecuteRequest {
                root,
                mutations,
                action,
                sandbox: Some(sandbox),
            })
            .await
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "hashline.transaction commit failed: {error}"
                ))
            })?;
        bounded_execution_output(response)?
    } else {
        let mut response = turn_environment
            .environment
            .plan_hashline_transaction(HashlineTransactionPlanRequest {
                root,
                mutations,
                sandbox: Some(sandbox),
            })
            .await
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "hashline.transaction preview failed: {error}"
                ))
            })?;
        bounded_preview_serialization(&mut response.preview, "preview", serde_json::to_string)?
    };
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        output,
        Some(true),
    )))
}

#[cfg(test)]
#[path = "hashline_transaction_tests.rs"]
mod tests;
