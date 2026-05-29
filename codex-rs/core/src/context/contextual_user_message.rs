use codex_protocol::items::HookPromptItem;
use codex_protocol::items::is_contextual_user_fragment as protocol_is_contextual_user_fragment;
use codex_protocol::items::parse_hook_prompt_fragment;
use codex_protocol::models::ContentItem;

pub(crate) fn is_contextual_user_fragment(content_item: &ContentItem) -> bool {
    protocol_is_contextual_user_fragment(content_item)
}

pub(crate) fn parse_visible_hook_prompt_message(
    id: Option<&String>,
    content: &[ContentItem],
) -> Option<HookPromptItem> {
    let mut fragments = Vec::new();

    for content_item in content {
        let ContentItem::InputText { text } = content_item else {
            return None;
        };
        if let Some(fragment) = parse_hook_prompt_fragment(text) {
            fragments.push(fragment);
            continue;
        }
        if is_contextual_user_fragment(content_item) {
            continue;
        }
        return None;
    }

    if fragments.is_empty() {
        return None;
    }

    Some(HookPromptItem::from_fragments(id, fragments))
}

#[cfg(test)]
#[path = "contextual_user_message_tests.rs"]
mod tests;
