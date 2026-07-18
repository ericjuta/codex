use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::InputModality;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

use crate::util::error_or_panic;
use tracing::info;
use tracing::warn;

const IMAGE_CONTENT_OMITTED_PLACEHOLDER: &str =
    "image content omitted because you do not support image input";
const AUDIO_CONTENT_OMITTED_PLACEHOLDER: &str =
    "audio content omitted because you do not support audio input";
// Changing this value would change model-visible IDs and invalidate prompt caches.
const SYNTHETIC_OUTPUT_ID_NAMESPACE: Uuid = Uuid::from_u128(0x90d38d3e_6a5b_4d52_bfe2_2f1e634bfac4);

/// Repairs invalid call/output shapes that the set-based checks below cannot
/// catch: duplicate calls or outputs sharing one call id, and outputs recorded
/// ahead of their call. The Responses API binds outputs to calls one-to-one and
/// scans the input sequentially, so any of these shapes fails the whole request
/// with "No tool call found for function call output with call_id ...".
pub(crate) fn repair_call_output_pairs(items: &mut Vec<ResponseItem>) {
    dedup_duplicate_calls(items);
    dedup_duplicate_outputs(items);
    move_outputs_after_calls(items);
}

/// Returns the pairing call id when the item is a tool call.
fn call_id_of_call(item: &ResponseItem) -> Option<&str> {
    match item {
        ResponseItem::FunctionCall { call_id, .. }
        | ResponseItem::CustomToolCall { call_id, .. }
        | ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        }
        | ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => Some(call_id),
        _ => None,
    }
}

/// Returns the pairing call id when the item is a client-executed tool output.
fn call_id_of_output(item: &ResponseItem) -> Option<&str> {
    match item {
        // Server-executed tool search outputs are not paired client-side.
        ResponseItem::ToolSearchOutput { execution, .. } if execution == "server" => None,
        ResponseItem::FunctionCallOutput { call_id, .. }
        | ResponseItem::CustomToolCallOutput { call_id, .. }
        | ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => Some(call_id),
        _ => None,
    }
}

fn dedup_duplicate_calls(items: &mut Vec<ResponseItem>) {
    let mut seen: HashSet<String> = HashSet::new();
    items.retain(|item| match call_id_of_call(item) {
        Some(call_id) => {
            let first = seen.insert(call_id.to_string());
            if !first {
                warn!("dropping duplicate tool call for call id: {call_id}");
            }
            first
        }
        None => true,
    });
}

fn dedup_duplicate_outputs(items: &mut Vec<ResponseItem>) {
    let mut remaining: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        if let Some(call_id) = call_id_of_output(item) {
            *remaining.entry(call_id.to_string()).or_insert(0) += 1;
        }
    }
    // Keep the last occurrence per call id: later outputs carry the most
    // recent information for that call (e.g. a re-recorded final result).
    items.retain(|item| {
        let Some(call_id) = call_id_of_output(item) else {
            return true;
        };
        let Some(count) = remaining.get_mut(call_id) else {
            return true;
        };
        *count -= 1;
        let keep = *count == 0;
        if !keep {
            warn!("dropping duplicate tool output for call id: {call_id}");
        }
        keep
    });
}

fn move_outputs_after_calls(items: &mut Vec<ResponseItem>) {
    let call_positions: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| call_id_of_call(item).map(|id| (id.to_string(), idx)))
        .collect();
    let misordered = items.iter().enumerate().any(|(idx, item)| {
        call_id_of_output(item)
            .and_then(|call_id| call_positions.get(call_id))
            .is_some_and(|&call_idx| call_idx > idx)
    });
    if !misordered {
        return;
    }

    let old_items = std::mem::take(items);
    let mut deferred: HashMap<String, Vec<ResponseItem>> = HashMap::new();
    let mut reordered: Vec<ResponseItem> = Vec::with_capacity(old_items.len());
    for (idx, item) in old_items.into_iter().enumerate() {
        if let Some(call_id) = call_id_of_output(&item)
            && call_positions
                .get(call_id)
                .is_some_and(|&call_idx| call_idx > idx)
        {
            warn!("moving tool output after its call for call id: {call_id}");
            deferred.entry(call_id.to_string()).or_default().push(item);
            continue;
        }
        let call_id = call_id_of_call(&item).map(str::to_string);
        reordered.push(item);
        if let Some(call_id) = call_id
            && let Some(outputs) = deferred.remove(&call_id)
        {
            reordered.extend(outputs);
        }
    }
    *items = reordered;
}

pub(crate) fn ensure_call_outputs_present(items: &mut Vec<ResponseItem>) {
    let mut function_output_ids = HashSet::new();
    let mut tool_search_output_ids = HashSet::new();
    let mut custom_tool_output_ids = HashSet::new();
    for item in items.iter() {
        match item {
            ResponseItem::FunctionCallOutput { call_id, .. } => {
                function_output_ids.insert(call_id.as_str());
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                ..
            } => {
                tool_search_output_ids.insert(call_id.as_str());
            }
            ResponseItem::CustomToolCallOutput { call_id, .. } => {
                custom_tool_output_ids.insert(call_id.as_str());
            }
            _ => {}
        }
    }

    // Collect synthetic outputs to insert immediately after their calls.
    // Store the insertion position (index of call) alongside the item so
    // we can insert in reverse order and avoid index shifting.
    let mut missing_outputs_to_insert: Vec<(usize, ResponseItem)> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        match item {
            ResponseItem::FunctionCall { id, call_id, .. }
                if !function_output_ids.contains(call_id.as_str()) =>
            {
                info!("Function call output is missing for call id: {call_id}");
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::FunctionCallOutput {
                        id: synthetic_output_id("fco", id.as_deref()),
                        call_id: call_id.clone(),
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
            }
            ResponseItem::ToolSearchCall {
                id,
                call_id: Some(call_id),
                ..
            } if !tool_search_output_ids.contains(call_id.as_str()) => {
                info!("Tool search output is missing for call id: {call_id}");
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::ToolSearchOutput {
                        id: synthetic_output_id("tso", id.as_deref()),
                        call_id: Some(call_id.clone()),
                        status: "completed".to_string(),
                        execution: "client".to_string(),
                        tools: Vec::new(),
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
            }
            ResponseItem::CustomToolCall { id, call_id, .. }
                if !custom_tool_output_ids.contains(call_id.as_str()) =>
            {
                error_or_panic(format!(
                    "Custom tool call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::CustomToolCallOutput {
                        id: synthetic_output_id("ctco", id.as_deref()),
                        call_id: call_id.clone(),
                        name: None,
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
            }
            // LocalShellCall is represented in upstream streams by a FunctionCallOutput
            ResponseItem::LocalShellCall {
                id,
                call_id: Some(call_id),
                ..
            } if !function_output_ids.contains(call_id.as_str()) => {
                error_or_panic(format!(
                    "Local shell call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::FunctionCallOutput {
                        id: synthetic_output_id("fco", id.as_deref()),
                        call_id: call_id.clone(),
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
            }
            _ => {}
        }
    }
    drop((
        function_output_ids,
        tool_search_output_ids,
        custom_tool_output_ids,
    ));

    // Insert synthetic outputs in reverse index order to avoid re-indexing.
    for (idx, output_item) in missing_outputs_to_insert.into_iter().rev() {
        items.insert(idx + 1, output_item);
    }
}

/// Derives a stable ID for a prompt-only output from its source call's item ID.
///
/// Prompt normalization can run repeatedly without persisting its synthetic
/// outputs, so the namespace and name format must remain stable across retries
/// and resumes to preserve prompt-cache reuse. Returning `None` when the source
/// call has no ID preserves the legacy behavior for older history items.
fn synthetic_output_id(prefix: &str, item_id: Option<&str>) -> Option<String> {
    let source_id = item_id.filter(|id| !id.is_empty())?;
    let name = format!("{prefix}:{source_id}");
    Some(format!(
        "{prefix}_{}",
        Uuid::new_v5(&SYNTHETIC_OUTPUT_ID_NAMESPACE, name.as_bytes())
    ))
}

pub(crate) fn remove_orphan_outputs(items: &mut Vec<ResponseItem>) {
    let function_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::FunctionCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let tool_search_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                ..
            } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let local_shell_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let custom_tool_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::CustomToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    items.retain(|item| match item {
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            let has_match =
                function_call_ids.contains(call_id) || local_shell_call_ids.contains(call_id);
            if !has_match {
                error_or_panic(format!(
                    "Orphan function call output for call id: {call_id}"
                ));
            }
            has_match
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            let has_match = custom_tool_call_ids.contains(call_id);
            if !has_match {
                error_or_panic(format!(
                    "Orphan custom tool call output for call id: {call_id}"
                ));
            }
            has_match
        }
        ResponseItem::ToolSearchOutput { execution, .. } if execution == "server" => true,
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => {
            let has_match = tool_search_call_ids.contains(call_id);
            if !has_match {
                error_or_panic(format!("Orphan tool search output for call id: {call_id}"));
            }
            has_match
        }
        ResponseItem::ToolSearchOutput { call_id: None, .. } => true,
        _ => true,
    });
}

pub(crate) fn remove_corresponding_for(items: &mut Vec<ResponseItem>, item: &ResponseItem) {
    match item {
        ResponseItem::FunctionCall { call_id, .. } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            if let Some(pos) = items.iter().position(|i| {
                matches!(i, ResponseItem::FunctionCall { call_id: existing, .. } if existing == call_id)
            }) {
                items.remove(pos);
            } else if let Some(pos) = items.iter().position(|i| {
                matches!(i, ResponseItem::LocalShellCall { call_id: Some(existing), .. } if existing == call_id)
            }) {
                items.remove(pos);
            }
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::ToolSearchOutput {
                        call_id: Some(existing),
                        ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(
                items,
                |i| {
                    matches!(
                        i,
                        ResponseItem::ToolSearchCall {
                            call_id: Some(existing),
                            ..
                        } if existing == call_id
                    )
                },
            );
        }
        ResponseItem::CustomToolCall { call_id, .. } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::CustomToolCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            remove_first_matching(
                items,
                |i| matches!(i, ResponseItem::CustomToolCall { call_id: existing, .. } if existing == call_id),
            );
        }
        ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        _ => {}
    }
}

fn remove_first_matching<F>(items: &mut Vec<ResponseItem>, predicate: F)
where
    F: Fn(&ResponseItem) -> bool,
{
    if let Some(pos) = items.iter().position(predicate) {
        items.remove(pos);
    }
}

/// Strip image content from messages and tool outputs when the model does not support images.
/// When `input_modalities` contains `InputModality::Image`, no stripping is performed.
pub(crate) fn strip_images_when_unsupported(
    input_modalities: &[InputModality],
    items: &mut [ResponseItem],
) {
    let supports_images = input_modalities.contains(&InputModality::Image);
    if supports_images {
        return;
    }

    for item in items.iter_mut() {
        match item {
            ResponseItem::Message { content, .. } => {
                let mut normalized_content = Vec::with_capacity(content.len());
                for content_item in content.iter() {
                    match content_item {
                        ContentItem::InputImage { .. } => {
                            normalized_content.push(ContentItem::InputText {
                                text: IMAGE_CONTENT_OMITTED_PLACEHOLDER.to_string(),
                            });
                        }
                        _ => normalized_content.push(content_item.clone()),
                    }
                }
                *content = normalized_content;
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content_items) = output.content_items_mut() {
                    let mut normalized_content_items = Vec::with_capacity(content_items.len());
                    for content_item in content_items.iter() {
                        match content_item {
                            FunctionCallOutputContentItem::InputImage { .. } => {
                                normalized_content_items.push(
                                    FunctionCallOutputContentItem::InputText {
                                        text: IMAGE_CONTENT_OMITTED_PLACEHOLDER.to_string(),
                                    },
                                );
                            }
                            _ => normalized_content_items.push(content_item.clone()),
                        }
                    }
                    *content_items = normalized_content_items;
                }
            }
            ResponseItem::ImageGenerationCall { result, .. } => {
                result.clear();
            }
            _ => {}
        }
    }
}

/// Strip audio content from messages when the model does not support audio.
/// When `input_modalities` contains `InputModality::Audio`, no stripping is performed.
pub(crate) fn strip_audio_when_unsupported(
    input_modalities: &[InputModality],
    items: &mut [ResponseItem],
) {
    if input_modalities.contains(&InputModality::Audio) {
        return;
    }

    for item in items.iter_mut() {
        if let ResponseItem::Message { content, .. } = item {
            for content_item in content.iter_mut() {
                if matches!(content_item, ContentItem::InputAudio { .. }) {
                    *content_item = ContentItem::InputText {
                        text: AUDIO_CONTENT_OMITTED_PLACEHOLDER.to_string(),
                    };
                }
            }
        }
    }
}
