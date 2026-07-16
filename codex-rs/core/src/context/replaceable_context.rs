use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_utils_cache::sha1_digest;
use codex_utils_string::truncate_middle_with_token_budget;

use super::ContextualUserFragment;

const MAX_VALUE_TOKENS: usize = 1_000;
const START_PREFIX: &str = "<replaceable_context key=\"";
const START_SUFFIX: &str = "\">";
const END_MARKER: &str = "</replaceable_context>";

pub(crate) struct ReplaceableContextFragment {
    key: String,
    value: Option<String>,
}

impl ReplaceableContextFragment {
    pub(crate) fn new(key: impl AsRef<str>, value: Option<String>) -> Self {
        let key = sha1_digest(key.as_ref().as_bytes())
            .into_iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self { key, value }
    }
}

impl ContextualUserFragment for ReplaceableContextFragment {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> String {
        let value = self
            .value
            .as_deref()
            .map(|value| truncate_middle_with_token_budget(value, MAX_VALUE_TOKENS).0);
        format!(
            "{START_PREFIX}{}{START_SUFFIX}{}{END_MARKER}",
            self.key,
            value.as_deref().unwrap_or_default()
        )
    }
}

pub(crate) fn replaceable_context_key(item: &ResponseItem) -> Option<(&str, bool)> {
    let ResponseItem::Message { role, content, .. } = item else {
        return None;
    };
    if role != "developer" || content.len() != 1 {
        return None;
    }
    let ContentItem::InputText { text } = &content[0] else {
        return None;
    };
    let rest = text.strip_prefix(START_PREFIX)?;
    let (key, value) = rest.split_once(START_SUFFIX)?;
    let value = value.strip_suffix(END_MARKER)?;
    Some((key, !value.is_empty()))
}
