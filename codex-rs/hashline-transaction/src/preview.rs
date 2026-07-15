use std::io;
use std::io::Write;
use std::str;

use codex_utils_path_uri::PathUri;
use serde::Serialize;

use crate::CanonicalPathKey;
use crate::ExactBytesDigest;
use crate::PlanError;
use crate::PlanSummary;
use crate::PlannedMutation;
use crate::PlannedTransaction;
use crate::TransactionLimits;
use crate::limits::check_limit;

/// Bounded UTF-8 content included in a model-visible transaction preview.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewText {
    pub text: String,
    pub truncated: bool,
}

/// Model-visible description of one planned filesystem mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum MutationPreview {
    Create {
        path: String,
        after_digest: ExactBytesDigest,
        content: PreviewText,
    },
    Update {
        path: String,
        before_digest: ExactBytesDigest,
        after_digest: ExactBytesDigest,
        content: PreviewText,
    },
    Delete {
        path: String,
        before_digest: ExactBytesDigest,
        content: PreviewText,
    },
    Move {
        source: String,
        destination: String,
        before_digest: ExactBytesDigest,
        after_digest: ExactBytesDigest,
        content: PreviewText,
    },
}

/// Bounded, serializable projection of an executor-owned transaction plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub environment_id: String,
    pub root: PathUri,
    pub plan_digest: ExactBytesDigest,
    pub summary: PlanSummary,
    pub preview_bytes: u64,
    pub preview_truncated: bool,
    pub mutations: Vec<MutationPreview>,
}

/// Builds the model-visible projection using limits selected by trusted orchestration.
pub fn build_preview<R, P>(
    plan: &PlannedTransaction<R, P>,
    limits: TransactionLimits,
) -> Result<PlanPreview, PlanError> {
    let mut ordered = plan.mutations.iter().collect::<Vec<_>>();
    ordered.sort_by(|first, second| preview_key(first).cmp(preview_key(second)));

    let mut content_sources = Vec::with_capacity(ordered.len());
    let mut mutations = Vec::with_capacity(ordered.len());
    for mutation in ordered {
        let preview = match mutation {
            PlannedMutation::Create {
                model_path,
                contents,
                after_digest,
                ..
            } => {
                content_sources.push(ContentSource {
                    path: model_path,
                    contents,
                });
                MutationPreview::Create {
                    path: model_path.clone(),
                    after_digest: *after_digest,
                    content: empty_preview_text(contents),
                }
            }
            PlannedMutation::Update {
                model_path,
                before,
                contents,
                after_digest,
                ..
            } => {
                content_sources.push(ContentSource {
                    path: model_path,
                    contents,
                });
                MutationPreview::Update {
                    path: model_path.clone(),
                    before_digest: before.exact_digest,
                    after_digest: *after_digest,
                    content: empty_preview_text(contents),
                }
            }
            PlannedMutation::Delete {
                model_path, before, ..
            } => {
                content_sources.push(ContentSource {
                    path: model_path,
                    contents: &before.contents,
                });
                MutationPreview::Delete {
                    path: model_path.clone(),
                    before_digest: before.exact_digest,
                    content: empty_preview_text(&before.contents),
                }
            }
            PlannedMutation::Move {
                model_source,
                before,
                model_destination,
                contents,
                after_digest,
                ..
            } => {
                content_sources.push(ContentSource {
                    path: model_source,
                    contents,
                });
                MutationPreview::Move {
                    source: model_source.clone(),
                    destination: model_destination.clone(),
                    before_digest: before.exact_digest,
                    after_digest: *after_digest,
                    content: empty_preview_text(contents),
                }
            }
        };
        mutations.push(preview);
    }

    let total_nonempty = content_sources
        .iter()
        .filter(|source| !source.contents.is_empty())
        .count() as u64;
    let mut preview = PlanPreview {
        environment_id: plan.environment_id.clone(),
        root: plan.root_uri.clone(),
        plan_digest: plan.plan_digest,
        summary: plan.summary,
        preview_bytes: 0,
        preview_truncated: total_nonempty != 0,
        mutations,
    };
    let base_response_bytes = serialized_len(&preview)?;
    check_limit(
        "response bytes",
        base_response_bytes,
        limits.max_response_bytes,
    )?;

    let mut budget = PreviewBudget {
        remaining_raw_bytes: limits.max_preview_bytes,
        response_limit: limits.max_response_bytes,
        base_response_bytes,
        encoded_content_bytes: 0,
        preview_bytes: 0,
        completed_nonempty: 0,
        total_nonempty,
    };
    for (mutation, source) in preview.mutations.iter_mut().zip(content_sources) {
        *preview_content_mut(mutation) = budget.take(source)?;
    }
    preview.preview_bytes = budget.preview_bytes;
    preview.preview_truncated = budget.completed_nonempty != budget.total_nonempty;
    check_limit(
        "response bytes",
        serialized_len(&preview)?,
        limits.max_response_bytes,
    )?;
    Ok(preview)
}

struct ContentSource<'a> {
    path: &'a str,
    contents: &'a [u8],
}

struct PreviewBudget {
    remaining_raw_bytes: u64,
    response_limit: u64,
    base_response_bytes: u64,
    encoded_content_bytes: u64,
    preview_bytes: u64,
    completed_nonempty: u64,
    total_nonempty: u64,
}

impl PreviewBudget {
    fn take(&mut self, source: ContentSource<'_>) -> Result<PreviewText, PlanError> {
        let contents = str::from_utf8(source.contents).map_err(|_| PlanError::InvalidUtf8 {
            path: source.path.to_string(),
        })?;
        if contents.is_empty() {
            return Ok(PreviewText {
                text: String::new(),
                truncated: false,
            });
        }

        let mut end = 0;
        for character in contents.chars() {
            let raw_bytes = character.len_utf8() as u64;
            if raw_bytes > self.remaining_raw_bytes {
                break;
            }
            let candidate_preview_bytes = self.preview_bytes.saturating_add(raw_bytes);
            let candidate_encoded_content_bytes = self
                .encoded_content_bytes
                .saturating_add(json_encoded_char_bytes(character));
            let completes_content = end + character.len_utf8() == contents.len();
            let candidate_completed_nonempty = self
                .completed_nonempty
                .saturating_add(u64::from(completes_content));
            if self.response_bytes(
                candidate_preview_bytes,
                candidate_encoded_content_bytes,
                candidate_completed_nonempty,
            ) > self.response_limit
            {
                break;
            }

            end += character.len_utf8();
            self.remaining_raw_bytes -= raw_bytes;
            self.preview_bytes = candidate_preview_bytes;
            self.encoded_content_bytes = candidate_encoded_content_bytes;
            self.completed_nonempty = candidate_completed_nonempty;
        }

        Ok(PreviewText {
            text: contents[..end].to_string(),
            truncated: end != contents.len(),
        })
    }

    fn response_bytes(
        &self,
        preview_bytes: u64,
        encoded_content_bytes: u64,
        completed_nonempty: u64,
    ) -> u64 {
        let preview_byte_digits = decimal_digits(preview_bytes).saturating_sub(1);
        let completed_all =
            u64::from(self.total_nonempty != 0 && completed_nonempty == self.total_nonempty);
        self.base_response_bytes
            .saturating_add(encoded_content_bytes)
            .saturating_add(preview_byte_digits)
            .saturating_add(completed_nonempty)
            .saturating_add(completed_all)
    }
}

fn empty_preview_text(contents: &[u8]) -> PreviewText {
    PreviewText {
        text: String::new(),
        truncated: !contents.is_empty(),
    }
}

fn preview_content_mut(preview: &mut MutationPreview) -> &mut PreviewText {
    match preview {
        MutationPreview::Create { content, .. }
        | MutationPreview::Update { content, .. }
        | MutationPreview::Delete { content, .. }
        | MutationPreview::Move { content, .. } => content,
    }
}

fn json_encoded_char_bytes(character: char) -> u64 {
    match character {
        '"' | '\\' | '\u{0008}' | '\u{0009}' | '\u{000a}' | '\u{000c}' | '\u{000d}' => 2,
        '\u{0000}'..='\u{001f}' => 6,
        _ => character.len_utf8() as u64,
    }
}

fn decimal_digits(mut value: u64) -> u64 {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn serialized_len(value: &impl Serialize) -> Result<u64, PlanError> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_err(|error| {
        PlanError::PreviewSerialization {
            reason: error.to_string(),
        }
    })?;
    Ok(counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: u64,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn preview_key<P>(mutation: &PlannedMutation<P>) -> &CanonicalPathKey {
    match mutation {
        PlannedMutation::Create { path_key, .. }
        | PlannedMutation::Update { path_key, .. }
        | PlannedMutation::Delete { path_key, .. } => path_key,
        PlannedMutation::Move { source_key, .. } => source_key,
    }
}
