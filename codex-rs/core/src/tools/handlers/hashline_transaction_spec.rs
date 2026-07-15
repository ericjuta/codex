use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde_json::json;

use super::hashline_transaction::TOOL_NAME;

pub(super) fn transaction_tool_spec(multi_environment: bool) -> ResponsesApiTool {
    let mut properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::one_of(
                vec![
                    tagged_object("preview", BTreeMap::new(), Vec::new()),
                    tagged_object("commit", BTreeMap::new(), Vec::new()),
                    tagged_object(
                        "commitPreviewed",
                        BTreeMap::from([(
                            "expectedPlanDigest".to_string(),
                            JsonSchema::string(Some(
                                "Plan digest returned by the preview that must still match."
                                    .to_string(),
                            )),
                        )]),
                        vec!["expectedPlanDigest".to_string()],
                    ),
                ],
                Some("Preview, commit immediately, or commit only a matching preview.".to_string()),
            ),
        ),
        (
            "root".to_string(),
            JsonSchema::string(Some(
                "Transaction root relative to the selected environment cwd. Defaults to the cwd."
                    .to_string(),
            )),
        ),
        (
            "mutations".to_string(),
            JsonSchema::array(
                mutation_schema(),
                Some(
                    "Ordered transaction mutations; execution canonicalizes path order."
                        .to_string(),
                ),
            ),
        ),
    ]);
    if multi_environment {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Environment id to target when multiple environments are attached.".to_string(),
            )),
        );
    }
    ResponsesApiTool {
        name: TOOL_NAME.to_string(),
        description: "Plan or recoverably commit a bounded multi-file Hashline transaction inside the selected environment."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: closed_object(
            properties,
            vec!["action".to_string(), "mutations".to_string()],
        ),
        output_schema: None,
    }
}

fn mutation_schema() -> JsonSchema {
    let expected = expected_file_schema();
    let edits = JsonSchema::array(file_edit_schema(), None);
    JsonSchema::one_of(
        vec![
            tagged_object(
                "create",
                BTreeMap::from([
                    ("path".to_string(), model_path_schema()),
                    ("contents".to_string(), contents_schema()),
                ]),
                vec!["path".to_string(), "contents".to_string()],
            ),
            tagged_object(
                "update",
                BTreeMap::from([
                    ("path".to_string(), model_path_schema()),
                    ("expected".to_string(), expected.clone()),
                    ("edits".to_string(), edits.clone()),
                ]),
                vec![
                    "path".to_string(),
                    "expected".to_string(),
                    "edits".to_string(),
                ],
            ),
            tagged_object(
                "delete",
                BTreeMap::from([
                    ("path".to_string(), model_path_schema()),
                    ("expected".to_string(), expected.clone()),
                ]),
                vec!["path".to_string(), "expected".to_string()],
            ),
            tagged_object(
                "move",
                BTreeMap::from([
                    ("source".to_string(), model_path_schema()),
                    ("expected".to_string(), expected),
                    ("destination".to_string(), model_path_schema()),
                    ("edits".to_string(), edits),
                ]),
                vec![
                    "source".to_string(),
                    "expected".to_string(),
                    "destination".to_string(),
                    "edits".to_string(),
                ],
            ),
        ],
        Some("One tagged create, update, delete, or move operation.".to_string()),
    )
}

fn file_edit_schema() -> JsonSchema {
    let lines = JsonSchema::array(JsonSchema::string(None), None);
    JsonSchema::one_of(
        vec![
            tagged_object(
                "replaceAll",
                BTreeMap::from([("contents".to_string(), contents_schema())]),
                vec!["contents".to_string()],
            ),
            tagged_object(
                "replaceLines",
                BTreeMap::from([
                    ("range".to_string(), line_range_schema()),
                    ("lines".to_string(), lines.clone()),
                ]),
                vec!["range".to_string(), "lines".to_string()],
            ),
            tagged_object(
                "insertBefore",
                BTreeMap::from([
                    ("anchor".to_string(), line_anchor_schema()),
                    ("lines".to_string(), lines.clone()),
                ]),
                vec!["anchor".to_string(), "lines".to_string()],
            ),
            tagged_object(
                "insertAfter",
                BTreeMap::from([
                    ("anchor".to_string(), line_anchor_schema()),
                    ("lines".to_string(), lines),
                ]),
                vec!["anchor".to_string(), "lines".to_string()],
            ),
        ],
        Some("One tagged exact-content or hash-anchored line edit.".to_string()),
    )
}

fn expected_file_schema() -> JsonSchema {
    closed_object(
        BTreeMap::from([(
            "exactDigest".to_string(),
            JsonSchema::string(Some(
                "Expected 64-character SHA-256 digest of the exact file bytes.".to_string(),
            )),
        )]),
        vec!["exactDigest".to_string()],
    )
}

fn line_anchor_schema() -> JsonSchema {
    closed_object(
        BTreeMap::from([
            (
                "line".to_string(),
                JsonSchema::integer(Some("1-indexed source line.".to_string())),
            ),
            (
                "expectedHash".to_string(),
                JsonSchema::string(Some("Expected four-hex Hashline line hash.".to_string())),
            ),
        ]),
        vec!["line".to_string(), "expectedHash".to_string()],
    )
}

fn line_range_schema() -> JsonSchema {
    closed_object(
        BTreeMap::from([
            ("start".to_string(), line_anchor_schema()),
            ("end".to_string(), line_anchor_schema()),
        ]),
        vec!["start".to_string(), "end".to_string()],
    )
}

fn model_path_schema() -> JsonSchema {
    JsonSchema::string(Some(
        "Path relative to the transaction root in the selected environment.".to_string(),
    ))
}

fn contents_schema() -> JsonSchema {
    JsonSchema::string(Some(
        "Exact UTF-8 file contents; no newline normalization is applied.".to_string(),
    ))
}

fn tagged_object(
    tag: &str,
    mut properties: BTreeMap<String, JsonSchema>,
    mut required: Vec<String>,
) -> JsonSchema {
    properties.insert(
        "type".to_string(),
        JsonSchema::string_enum(vec![json!(tag)], None),
    );
    required.insert(0, "type".to_string());
    closed_object(properties, required)
}

fn closed_object(properties: BTreeMap<String, JsonSchema>, required: Vec<String>) -> JsonSchema {
    JsonSchema::object(
        properties,
        Some(required),
        /*additional_properties*/ Some(false.into()),
    )
}
