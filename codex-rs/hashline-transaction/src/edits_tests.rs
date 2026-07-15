use pretty_assertions::assert_eq;

use super::edits::EditOutputLimit;
use super::edits::compile_edits as compile_edits_with_limit;
use super::edits::line_hash;
use super::*;

fn anchor(line: u64, text: &str) -> LineAnchor {
    LineAnchor {
        line,
        expected_hash: line_hash(text),
    }
}

fn compile_edits(
    path: &str,
    before: &[u8],
    edits: Vec<FileEdit>,
    allow_empty: bool,
) -> Result<Vec<u8>, PlanError> {
    compile_edits_with_limit(
        path,
        before,
        edits,
        allow_empty,
        EditOutputLimit {
            max_bytes: u64::MAX,
        },
    )
}

#[test]
fn ordered_line_edits_preserve_bom_crlf_and_final_newline_state() {
    let before = "\u{feff}alpha\r\nbeta\r\ngamma".as_bytes();
    let after = compile_edits(
        "file",
        before,
        vec![
            FileEdit::InsertAfter {
                anchor: anchor(1, "alpha"),
                lines: vec!["one".to_string()],
            },
            FileEdit::ReplaceLines {
                range: LineRange {
                    start: anchor(2, "beta"),
                    end: anchor(2, "beta"),
                },
                lines: vec!["B".to_string()],
            },
            FileEdit::InsertBefore {
                anchor: anchor(3, "gamma"),
                lines: vec!["two".to_string()],
            },
            FileEdit::InsertAfter {
                anchor: anchor(3, "gamma"),
                lines: vec!["omega".to_string()],
            },
        ],
        false,
    )
    .unwrap();

    assert_eq!(
        after,
        "\u{feff}alpha\r\none\r\nB\r\ntwo\r\ngamma\r\nomega".as_bytes()
    );
}

#[test]
fn replace_lines_can_delete_a_range_without_normalizing_other_bytes() {
    let after = compile_edits(
        "file",
        b"alpha\rbeta\rgamma\r",
        vec![FileEdit::ReplaceLines {
            range: LineRange {
                start: anchor(2, "beta"),
                end: anchor(3, "gamma"),
            },
            lines: Vec::new(),
        }],
        false,
    )
    .unwrap();

    assert_eq!(after, b"alpha\r");
}

#[test]
fn cardinality_changes_preserve_mixed_source_line_endings() {
    let after = compile_edits(
        "file",
        b"alpha\r\nbeta\ngamma\rdelta",
        vec![FileEdit::ReplaceLines {
            range: LineRange {
                start: anchor(2, "beta"),
                end: anchor(2, "beta"),
            },
            lines: vec!["B1".to_string(), "B2".to_string()],
        }],
        false,
    )
    .unwrap();

    assert_eq!(after, b"alpha\r\nB1\r\nB2\ngamma\rdelta");
}

#[test]
fn repeated_insert_after_preserves_request_order_and_final_newline_state() {
    let after = compile_edits(
        "file",
        b"alpha",
        vec![
            FileEdit::InsertAfter {
                anchor: anchor(1, "alpha"),
                lines: vec!["first".to_string()],
            },
            FileEdit::InsertAfter {
                anchor: anchor(1, "alpha"),
                lines: vec!["second".to_string()],
            },
        ],
        false,
    )
    .unwrap();

    assert_eq!(after, b"alpha\nfirst\nsecond");
}

#[test]
fn rejects_stale_reused_and_malformed_anchors() {
    let expected_hash = "0000".to_string();
    let actual_hash = line_hash("alpha");
    assert_eq!(
        compile_edits(
            "file",
            b"alpha\nbeta\n",
            vec![FileEdit::InsertBefore {
                anchor: LineAnchor {
                    line: 1,
                    expected_hash: expected_hash.clone(),
                },
                lines: vec!["new".to_string()],
            }],
            false,
        ),
        Err(PlanError::InvalidAnchor {
            path: "file".to_string(),
            reason: format!("line 1 hash mismatch: expected {expected_hash}, found {actual_hash}"),
        })
    );

    let replaced = FileEdit::ReplaceLines {
        range: LineRange {
            start: anchor(1, "alpha"),
            end: anchor(1, "alpha"),
        },
        lines: vec!["new".to_string()],
    };
    assert_eq!(
        compile_edits("file", b"alpha\n", vec![replaced.clone(), replaced], false,),
        Err(PlanError::InvalidAnchor {
            path: "file".to_string(),
            reason: "line 1 was already replaced".to_string(),
        })
    );

    assert_eq!(
        compile_edits(
            "file",
            b"alpha\n",
            vec![FileEdit::InsertAfter {
                anchor: LineAnchor {
                    line: 1,
                    expected_hash: "xyz".to_string(),
                },
                lines: vec!["new".to_string()],
            }],
            false,
        ),
        Err(PlanError::InvalidAnchor {
            path: "file".to_string(),
            reason: "line 1 hash must contain exactly 4 hexadecimal characters".to_string(),
        })
    );
}

#[test]
fn rejects_non_utf8_files_and_embedded_line_endings() {
    assert_eq!(
        compile_edits(
            "file",
            &[0xff],
            vec![FileEdit::ReplaceAll {
                contents: b"valid".to_vec(),
            }],
            false,
        ),
        Err(PlanError::InvalidUtf8 {
            path: "file".to_string(),
        })
    );
    assert_eq!(
        compile_edits(
            "file",
            b"alpha\n",
            vec![FileEdit::InsertBefore {
                anchor: anchor(1, "alpha"),
                lines: vec!["two\nlines".to_string()],
            }],
            false,
        ),
        Err(PlanError::InvalidEditText {
            path: "file".to_string(),
            reason: "line values must not contain line endings".to_string(),
        })
    );
}

#[test]
fn rejects_oversized_output_before_allocating_the_encoded_buffer() {
    assert_eq!(
        compile_edits_with_limit(
            "file",
            b"alpha\n",
            vec![FileEdit::InsertAfter {
                anchor: anchor(1, "alpha"),
                lines: vec!["beta".to_string()],
            }],
            false,
            EditOutputLimit { max_bytes: 10 },
        ),
        Err(PlanError::Limit {
            resource: "file bytes",
            observed: 11,
            limit: 10,
        })
    );
}
