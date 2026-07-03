use super::apply_hashline_patch_or_create_empty;
use super::build_hashline_patch_success_body;
use super::build_hashline_read_body;
use super::hashline_block::find_block_span;
use super::hashline_block::language_for_path;
use super::hashline_hash::hash_hex;
use super::hashline_hash::line_hash;
use super::hashline_patch::HashlinePatchFileMutation;
use super::hashline_patch::HashlinePatchFileOperation;
use super::hashline_patch::HashlinePatchFileUpdate;
use super::hashline_patch::HashlinePatchSection;
use super::hashline_patch::apply_hashline_patch;
use super::hashline_patch::apply_patch_for_hashline_mutations;
use super::hashline_patch::apply_patch_for_hashline_remove;
use super::hashline_patch::apply_patch_for_hashline_rename;
use super::hashline_patch::apply_patch_for_hashline_update;
use super::hashline_patch::apply_patch_for_hashline_updates;
use super::hashline_patch::build_hashline_patch_preview;
use super::hashline_patch::hashline_patch_is_aborted;
use super::hashline_patch::parse_hashline_patch_file_operation;
use super::hashline_patch::split_hashline_patch_sections;
use super::resolve_find_block_anchor;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn applies_basic_line_operations() {
    let original = "alpha\nbeta\ngamma\n";
    let patch = format!(
        "SWAP 2:{}|bravo\nINS.POST 3:{}|delta\nDEL 1:{}",
        line_hash("beta"),
        line_hash("gamma"),
        line_hash("alpha")
    );

    let updated = apply_hashline_patch("notes.txt", original, &patch).expect("patch should apply");

    assert_eq!(updated, "bravo\ngamma\ndelta\n");
}

#[test]
fn rejects_stale_line_hash() {
    let error = apply_hashline_patch("notes.txt", "alpha\n", "SWAP 1:00|omega")
        .expect_err("stale hash should be rejected");

    assert!(
        error.to_string().contains("hash mismatch"),
        "unexpected error: {error}"
    );
}

#[test]
fn accepts_single_hex_line_hash_anchor() {
    let original_line = (0..10_000)
        .map(|index| format!("candidate {index}"))
        .find(|line| {
            let hash = line_hash(line);
            hash.starts_with('0') && hash != "00"
        })
        .expect("test fixture should find a line with a one-digit hash value");
    let full_hash = line_hash(&original_line);
    let short_hash = full_hash
        .strip_prefix('0')
        .expect("fixture hash should start with zero");
    let original = format!("{original_line}\n");
    let patch = format!("SWAP 1:{short_hash}|omega");

    let updated = apply_hashline_patch("notes.txt", &original, &patch)
        .expect("one-hex line hash should validate as the same numeric hash");

    assert_eq!(updated, "omega\n");
}

#[test]
fn accepts_compact_head_and_tail_insert_syntax() {
    let updated = apply_hashline_patch("notes.txt", "middle\n", "INS.HEAD|top\nINS.TAIL|bottom")
        .expect("compact insert syntax should apply");

    assert_eq!(updated, "top\nmiddle\nbottom\n");
}

#[test]
fn line_hash_matches_reference_trailing_whitespace_behavior() {
    assert_eq!(
        line_hash("return decoded"),
        line_hash("return decoded   \r")
    );
    assert_ne!(line_hash("  return decoded"), line_hash("return decoded"));
}

#[test]
fn file_hash_matches_reference_normalization_behavior() {
    assert_eq!(
        hash_hex("alpha\r\nbeta\r\n", 4),
        hash_hex("alpha\nbeta\n", 4)
    );
    assert_eq!(hash_hex("\u{feff}alpha\n", 4), hash_hex("alpha\n", 4));
}

#[test]
fn read_body_without_end_line_only_truncates_when_capped() {
    let contents = "alpha\nbeta\n";
    let body = build_hashline_read_body(
        "notes.txt",
        contents,
        /*start_line*/ 1,
        /*requested_end_line*/ None,
        /*max_lines*/ 200,
    );

    assert_eq!(
        body,
        json!({
            "path": "notes.txt",
            "hash": hash_hex(contents, 4),
            "header": format!("[notes.txt#{}]", hash_hex(contents, 4)),
            "start_line": 1,
            "end_line": 2,
            "total_lines": 2,
            "truncated": false,
            "next_start_line": null,
            "content": format!("1:{}|alpha\n2:{}|beta", line_hash("alpha"), line_hash("beta")),
            "lines": [
                {
                    "n": 1,
                    "hash": line_hash("alpha"),
                    "content": "alpha",
                },
                {
                    "n": 2,
                    "hash": line_hash("beta"),
                    "content": "beta",
                },
            ],
        })
    );
}

#[test]
fn patch_accepts_matching_file_header() {
    let original = "alpha\nbeta\ngamma\n";
    let patch = format!(
        "[notes.txt#{}]\nSWAP 2:{}|bravo",
        hash_hex(original, 4),
        line_hash("beta")
    );

    let updated = apply_hashline_patch("notes.txt", original, &patch).expect("patch should apply");

    assert_eq!(updated, "alpha\nbravo\ngamma\n");
}

#[test]
fn patch_accepts_bracketed_apply_patch_style_file_header() {
    let original = "alpha\nbeta\ngamma\n";
    let patch = format!(
        "[*** Update File: notes.txt#{}]\nSWAP 2:{}|bravo",
        hash_hex(original, 4),
        line_hash("beta")
    );

    let updated = apply_hashline_patch("notes.txt", original, &patch)
        .expect("bracketed apply_patch-style header should recover");

    assert_eq!(updated, "alpha\nbravo\ngamma\n");
}

#[test]
fn patch_accepts_repeated_matching_file_sections() {
    let original = "alpha\nbeta\ngamma\n";
    let file_hash = hash_hex(original, 4);
    let patch = format!(
        "[notes.txt#{file_hash}]\nSWAP 2:{}|bravo\n[notes.txt#{file_hash}]\nINS.TAIL:\n+delta",
        line_hash("beta")
    );

    let updated = apply_hashline_patch("notes.txt", original, &patch).expect("patch should apply");

    assert_eq!(updated, "alpha\nbravo\ngamma\ndelta\n");
}

#[test]
fn patch_sections_group_targets_for_multi_file_handler() {
    let patch = "[a.txt#1111]\nSWAP 1:\n+uno\n[b.txt#2222]\nDEL 2\n[a.txt#1111]\nINS.TAIL:\n+tres";

    let sections = split_hashline_patch_sections("a.txt", patch).expect("sections should parse");

    assert_eq!(
        sections,
        vec![
            HashlinePatchSection {
                path: "a.txt".to_string(),
                expected_hash: Some("1111".to_string()),
                patch: "SWAP 1:\n+uno\nINS.TAIL:\n+tres".to_string(),
            },
            HashlinePatchSection {
                path: "b.txt".to_string(),
                expected_hash: Some("2222".to_string()),
                patch: "DEL 2".to_string(),
            },
        ]
    );
}

#[test]
fn patch_rejects_conflicting_same_path_file_section_hashes() {
    let original = "alpha\nbeta\n";
    let actual_hash = hash_hex(original, 4);
    let stale_hash = if actual_hash == "0000" {
        "0001"
    } else {
        "0000"
    };
    let patch = format!(
        "[notes.txt#{actual_hash}]\nSWAP 1:{}|ALPHA\n[notes.txt#{stale_hash}]\nDEL 2:{}",
        line_hash("alpha"),
        line_hash("beta")
    );

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("conflicting section hashes should be rejected");

    assert!(
        error.to_string().contains("conflicting hash tags"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_sections_reject_conflicting_same_path_hashes() {
    let patch = "[a.txt#1111]\nSWAP 1:\n+uno\n[a.txt#2222]\nDEL 2";

    let error = split_hashline_patch_sections("a.txt", patch)
        .expect_err("conflicting section hashes should be rejected");

    assert!(
        error.to_string().contains("conflicting hash tags"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_rejects_stale_file_header() {
    let original = "alpha\nbeta\n";
    let actual_hash = hash_hex(original, 4);
    let stale_hash = if actual_hash == "0000" {
        "0001"
    } else {
        "0000"
    };
    let patch = format!("[notes.txt#{stale_hash}]\nDEL 2:{}", line_hash("beta"));

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("stale file hash should be rejected");

    assert!(
        error.to_string().contains("file hash mismatch"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_rejects_wrong_file_header_path() {
    let original = "alpha\nbeta\n";
    let patch = format!("[other.txt#{}]\nDEL 2", hash_hex(original, 4));

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("wrong file header path should be rejected");

    assert!(
        error.to_string().contains("does not match target path"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_sections_accept_optional_file_hashes() {
    let sections = split_hashline_patch_sections(
        "fallback.txt",
        "[created.txt]\nINS.TAIL:\n+new\n[existing.txt#abcd]\nDEL 1",
    )
    .expect("optional file-hash sections should parse");

    assert_eq!(
        sections,
        vec![
            HashlinePatchSection {
                path: "created.txt".to_string(),
                expected_hash: None,
                patch: "INS.TAIL:\n+new".to_string(),
            },
            HashlinePatchSection {
                path: "existing.txt".to_string(),
                expected_hash: Some("abcd".to_string()),
                patch: "DEL 1".to_string(),
            },
        ]
    );
}

#[test]
fn patch_sections_recover_bracketed_apply_patch_style_headers() {
    let sections = split_hashline_patch_sections(
        "fallback.txt",
        "[*** Add File: created.txt]\nINS.TAIL:\n+new\n[*** Update File: existing.txt#abcd]\nDEL 1\n[**Move to: moved.txt]\nINS.HEAD:\n+hi",
    )
    .expect("bracketed apply_patch-style headers should recover");

    assert_eq!(
        sections,
        vec![
            HashlinePatchSection {
                path: "created.txt".to_string(),
                expected_hash: None,
                patch: "INS.TAIL:\n+new".to_string(),
            },
            HashlinePatchSection {
                path: "existing.txt".to_string(),
                expected_hash: Some("abcd".to_string()),
                patch: "DEL 1".to_string(),
            },
            HashlinePatchSection {
                path: "moved.txt".to_string(),
                expected_hash: None,
                patch: "INS.HEAD:\n+hi".to_string(),
            },
        ]
    );
}

#[test]
fn single_file_applier_rejects_multi_file_sections_with_clear_message() {
    let original = "alpha\nbeta\n";
    let patch = format!(
        "[notes.txt#{}]\nDEL 2\n[other.txt#0000]\nDEL 1",
        hash_hex(original, 4)
    );

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("multi-file sections should be rejected");

    assert!(
        error.to_string().contains("single-file patch application"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_rejects_apply_patch_contamination() {
    let error = apply_hashline_patch(
        "notes.txt",
        "alpha\n",
        "*** Begin Patch\n*** Update File: notes.txt\n@@\n-alpha\n+omega\n*** End Patch",
    )
    .expect_err("apply_patch syntax should be rejected");

    assert!(
        error.to_string().contains("apply_patch sentinel"),
        "unexpected error: {error}"
    );
}

#[test]
fn patch_rejects_unified_diff_contamination() {
    let error = apply_hashline_patch("notes.txt", "alpha\n", "@@ -1 +1 @@\n-alpha\n+omega")
        .expect_err("unified diff syntax should be rejected");

    assert!(
        error.to_string().contains("unified-diff hunk header"),
        "unexpected error: {error}"
    );
}

#[test]
fn applies_readme_style_swap_body() {
    let updated = apply_hashline_patch("notes.txt", "alpha\nbeta\ngamma\n", "SWAP 2:\n+bravo")
        .expect("README-style swap body should apply");

    assert_eq!(updated, "alpha\nbravo\ngamma\n");
}

#[test]
fn applies_bare_payload_lines() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\nbravo\ncharlie",
    )
    .expect("bare payload lines should apply");

    assert_eq!(updated, "alpha\nbravo\ncharlie\ngamma\n");
}

#[test]
fn strips_uniform_read_output_payload_prefixes() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\n1:aa|bravo\n2:bb|charlie",
    )
    .expect("pasted read output rows should apply");

    assert_eq!(updated, "alpha\nbravo\ncharlie\ngamma\n");
}

#[test]
fn strips_decorated_read_output_payload_prefixes() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\n>>> 1:aa|bravo\n>> * 2:bb|charlie\n  + 3:cc|delta",
    )
    .expect("decorated pasted read output rows should apply");

    assert_eq!(updated, "alpha\nbravo\ncharlie\ndelta\ngamma\n");
}

#[test]
fn keeps_mixed_read_output_payload_prefixes_literal() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\n1:aa|bravo\ncharlie",
    )
    .expect("mixed bare payload rows should stay literal");

    assert_eq!(updated, "alpha\n1:aa|bravo\ncharlie\ngamma\n");
}

#[test]
fn explicit_payload_rows_keep_read_output_prefixes_literal() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\n+1:aa|literal",
    )
    .expect("explicit payload row should stay literal");

    assert_eq!(updated, "alpha\n1:aa|literal\ngamma\n");
}

#[test]
fn bare_payload_stops_before_next_operation() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\ndelta\n",
        "SWAP 2:\nbravo\nDEL 4",
    )
    .expect("operation-looking rows should start the next operation");

    assert_eq!(updated, "alpha\nbravo\ngamma\n");
}

#[test]
fn rejects_minus_payload_rows() {
    let error = apply_hashline_patch("notes.txt", "alpha\nbeta\n", "SWAP 2:\n-bravo")
        .expect_err("minus rows should reject");

    assert!(
        error.to_string().contains("- rows are not accepted"),
        "unexpected error: {error}"
    );
}

#[test]
fn applies_escaped_payload_prefixes() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "SWAP 2:\n++literal plus\n+-literal minus",
    )
    .expect("escaped payload prefixes should apply");

    assert_eq!(updated, "alpha\n+literal plus\n-literal minus\ngamma\n");
}

#[test]
fn applies_readme_style_range_swap() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\ndelta\n",
        "SWAP 2..3:\n+bravo\n+charlie",
    )
    .expect("README-style range swap should apply");

    assert_eq!(updated, "alpha\nbravo\ncharlie\ndelta\n");
}

#[test]
fn applies_readme_style_hashed_range_anchors() {
    let original = "alpha\nbeta\ngamma\ndelta\n";
    let patch = format!(
        "SWAP 2:{}..3:{}:\n+bravo\n+charlie",
        line_hash("beta"),
        line_hash("gamma")
    );

    let updated = apply_hashline_patch("notes.txt", original, &patch)
        .expect("hashed range endpoints should apply");

    assert_eq!(updated, "alpha\nbravo\ncharlie\ndelta\n");
}

#[test]
fn accepts_reference_range_separators() {
    let spaced_equals = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\ngamma\ndelta\n",
        "SWAP 2 ..= 3:\n+bravo\n+charlie",
    )
    .expect("..= range separator should apply");
    let hyphen = apply_hashline_patch("notes.txt", "alpha\nbeta\ngamma\ndelta\n", "DEL 2-3")
        .expect("hyphen range separator should apply");

    assert_eq!(spaced_equals, "alpha\nbravo\ncharlie\ndelta\n");
    assert_eq!(hyphen, "alpha\ndelta\n");
}

#[test]
fn rejects_stale_range_end_hash() {
    let original = "alpha\nbeta\ngamma\ndelta\n";
    let patch = format!("DEL 2:{}..3:00", line_hash("beta"));

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("stale end hash should reject the range");

    assert!(
        error.to_string().contains("line 3 hash mismatch"),
        "unexpected error: {error}"
    );
}

#[test]
fn applies_readme_style_delete_range() {
    let updated = apply_hashline_patch("notes.txt", "alpha\nbeta\ngamma\ndelta\n", "DEL 2..3")
        .expect("README-style delete range should apply");

    assert_eq!(updated, "alpha\ndelta\n");
}

#[test]
fn applies_readme_style_insert_bodies() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "middle\n",
        "INS.HEAD:\n+top\nINS.POST 1:\n+after middle\nINS.TAIL:\n+bottom",
    )
    .expect("README-style insert bodies should apply");

    assert_eq!(updated, "top\nmiddle\nafter middle\nbottom\n");
}

#[test]
fn accepts_patch_envelope_markers() {
    let updated = apply_hashline_patch(
        "notes.txt",
        "alpha\nbeta\n",
        "*** Begin Patch\nSWAP 2:\n+bravo\n*** End Patch",
    )
    .expect("Hashline patch envelope should be ignored");

    assert_eq!(updated, "alpha\nbravo\n");
}

#[test]
fn abort_marker_suppresses_hashline_patch() {
    let patch = "*** Begin Patch\nSWAP 2:\n+bravo\n*** Abort\n*** End Patch";
    let updated = apply_hashline_patch("notes.txt", "alpha\nbeta\n", patch)
        .expect("abort marker should suppress the embedded patch");

    assert!(hashline_patch_is_aborted(patch));
    assert_eq!(updated, "alpha\nbeta\n");
}

#[test]
fn subsequent_operations_use_original_line_anchors() {
    let original = "AAA\nBBB\nCCC\nDDD\n";
    let patch = format!("INS.POST 1:\n+XXX\nSWAP 4:{}:\n+ZZZ", line_hash("DDD"));

    let updated = apply_hashline_patch("notes.txt", original, &patch)
        .expect("later operation should target original line after earlier insert");

    assert_eq!(updated, "AAA\nXXX\nBBB\nCCC\nZZZ\n");
}

#[test]
fn applies_swap_block_operation() {
    let original = "fn hello() {\n    let x = 1;\n}\n";

    let updated = apply_hashline_patch(
        "src/main.rs",
        original,
        "SWAP.BLK 1:\n+fn replaced() {\n+    let y = 2;\n+}",
    )
    .expect("SWAP.BLK should replace the resolved block");

    assert_eq!(updated, "fn replaced() {\n    let y = 2;\n}\n");
}

#[test]
fn applies_delete_block_operation() {
    let original = "fn hello() {\n    let x = 1;\n}\nfn keep() {}\n";

    let updated = apply_hashline_patch("src/main.rs", original, "DEL.BLK 1")
        .expect("DEL.BLK should delete the resolved block");

    assert_eq!(updated, "fn keep() {}\n");
}

#[test]
fn applies_insert_block_post_operation() {
    let original = "fn hello() {\n    let x = 1;\n}\n";

    let updated = apply_hashline_patch(
        "src/main.rs",
        original,
        "INS.BLK.POST 1:\n+fn world() {\n+    let y = 2;\n+}",
    )
    .expect("INS.BLK.POST should insert after the resolved block");

    assert_eq!(
        updated,
        "fn hello() {\n    let x = 1;\n}\nfn world() {\n    let y = 2;\n}\n"
    );
}

#[test]
fn applies_insert_block_alias_operation() {
    let original = "fn hello() {\n    let x = 1;\n}\n";

    let updated = apply_hashline_patch(
        "src/main.rs",
        original,
        "INS.BLK 1:\n+fn world() {\n+    let y = 2;\n+}",
    )
    .expect("INS.BLK should alias INS.BLK.POST");

    assert_eq!(
        updated,
        "fn hello() {\n    let x = 1;\n}\nfn world() {\n    let y = 2;\n}\n"
    );
}

#[test]
fn applies_insert_block_pre_operation() {
    let original = "fn hello() {\n    let x = 1;\n}\n";

    let updated = apply_hashline_patch(
        "src/main.rs",
        original,
        "INS.BLK.PRE 1:\n+fn preamble() {\n+}",
    )
    .expect("INS.BLK.PRE should insert before the resolved block");

    assert_eq!(
        updated,
        "fn preamble() {\n}\nfn hello() {\n    let x = 1;\n}\n"
    );
}

#[test]
fn applies_python_swap_block_header_operation() {
    let original = "def hello():\n    x = 1\n    return x\n\ndef keep():\n    return 2\n";

    let updated = apply_hashline_patch(
        "src/main.py",
        original,
        "SWAP.BLK 1:\n+def hello():\n+    return 42",
    )
    .expect("Python header block should replace only the anchored suite");

    assert_eq!(
        updated,
        "def hello():\n    return 42\n\ndef keep():\n    return 2\n"
    );
}

#[test]
fn applies_ruby_swap_block_header_operation() {
    let original = "def hello\n  x = 1\nend\n\ndef keep\n  2\nend\n";

    let updated = apply_hashline_patch(
        "src/main.rb",
        original,
        "SWAP.BLK 1:\n+def hello\n+  42\n+end",
    )
    .expect("Ruby block should replace only the anchored method");

    assert_eq!(updated, "def hello\n  42\nend\n\ndef keep\n  2\nend\n");
}

#[test]
fn generated_update_patch_is_localized() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
    let updated = "one\ntwo\nthree\nfour\nFIVE\nsix\nseven\neight\nnine\n";

    let patch = apply_patch_for_hashline_update(
        "notes.txt",
        original,
        updated,
        /*create*/ false,
        /*environment_id*/ None,
    )
    .expect("patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Update File: notes.txt\n@@\n two\n three\n four\n-five\n+FIVE\n six\n seven\n eight\n*** End Patch"
    );
}

#[test]
fn dry_run_preview_reports_changed_lines() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\n";
    let updated = "one\ntwo\nTHREE\nfour\nFIVE\nsix\n";

    let preview =
        build_hashline_patch_preview(original, updated).expect("preview should be generated");

    assert_eq!(preview.old_start_line, Some(3));
    assert_eq!(preview.old_end_line, Some(5));
    assert_eq!(preview.new_start_line, Some(3));
    assert_eq!(preview.new_end_line, Some(5));
    assert!(!preview.truncated);
    assert_eq!(
        preview.content,
        format!(
            "-3:{}|three\n-4:{}|four\n-5:{}|five\n+3:{}|THREE\n+4:{}|four\n+5:{}|FIVE",
            line_hash("three"),
            line_hash("four"),
            line_hash("five"),
            line_hash("THREE"),
            line_hash("four"),
            line_hash("FIVE")
        )
    );
}

#[test]
fn success_output_reports_fresh_hashline_excerpt() {
    let original = "alpha\nbeta\ngamma\n";
    let updated = "alpha\nbravo\ngamma\n";

    let output =
        build_hashline_patch_success_body("notes.txt", original, updated, /*create*/ false)
            .expect("success body should be generated");

    let new_hash = hash_hex(updated, 4);
    assert_eq!(
        output,
        serde_json::json!({
            "success": true,
            "path": "notes.txt",
            "header": format!("[notes.txt#{new_hash}]"),
            "operation": "update",
            "old_hash": hash_hex(original, 4),
            "new_hash": new_hash,
            "start_line": 2,
            "end_line": 2,
            "total_lines": 3,
            "truncated": false,
            "content": format!("2:{}|bravo", line_hash("bravo")),
            "preview": {
                "old_start_line": 2,
                "old_end_line": 2,
                "new_start_line": 2,
                "new_end_line": 2,
                "truncated": false,
                "content": format!(
                    "-2:{}|beta\n+2:{}|bravo",
                    line_hash("beta"),
                    line_hash("bravo")
                ),
            },
        })
    );
}

#[test]
fn generated_create_patch_uses_add_file() {
    let patch = apply_patch_for_hashline_update(
        "created.txt",
        "",
        "hello\nthere",
        /*create*/ true,
        /*environment_id*/ Some("env-1"),
    )
    .expect("create patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Environment ID: env-1\n*** Add File: created.txt\n+hello\n+there\n*** End Patch"
    );
}

#[test]
fn generated_empty_create_patch_uses_empty_add_file() {
    let patch = apply_patch_for_hashline_update(
        "empty.txt",
        "",
        "",
        /*create*/ true,
        /*environment_id*/ None,
    )
    .expect("empty create patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Add File: empty.txt\n*** End Patch"
    );
}

#[test]
fn create_empty_patch_accepts_no_operations() {
    let created = apply_hashline_patch_or_create_empty("empty.txt", "", "", /*create*/ true)
        .expect("empty create patch should apply");
    let update_error =
        apply_hashline_patch_or_create_empty("empty.txt", "", "", /*create*/ false)
            .expect_err("empty update patch should still reject missing operations");

    assert_eq!(created, "");
    assert_eq!(
        update_error.to_string(),
        "hashline.patch did not contain any operations"
    );
}

#[test]
fn success_output_reports_empty_create() {
    let output = build_hashline_patch_success_body("empty.txt", "", "", /*create*/ true)
        .expect("empty create success body should be generated");

    let empty_hash = hash_hex("", 4);
    assert_eq!(
        output,
        serde_json::json!({
            "success": true,
            "path": "empty.txt",
            "header": format!("[empty.txt#{empty_hash}]"),
            "operation": "create",
            "old_hash": empty_hash,
            "new_hash": empty_hash,
            "start_line": null,
            "end_line": null,
            "total_lines": 0,
            "truncated": false,
            "content": "",
            "preview": null,
        })
    );
}

#[test]
fn generated_multi_file_patch_uses_one_apply_patch_envelope() {
    let updates = [
        HashlinePatchFileUpdate {
            path: "a.txt",
            old_contents: "one\n",
            new_contents: "uno\n",
            create: false,
        },
        HashlinePatchFileUpdate {
            path: "b.txt",
            old_contents: "two\n",
            new_contents: "two\ntres\n",
            create: false,
        },
    ];
    let patch = apply_patch_for_hashline_updates(&updates, /*environment_id*/ Some("env-1"))
        .expect("multi-file patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Environment ID: env-1\n*** Update File: a.txt\n@@\n-one\n+uno\n*** Update File: b.txt\n@@\n two\n+tres\n*** End Patch"
    );
}

#[test]
fn file_operation_parser_accepts_remove_and_rename() {
    assert_eq!(
        parse_hashline_patch_file_operation("REM").expect("REM should parse"),
        Some(HashlinePatchFileOperation::Remove)
    );
    assert_eq!(
        parse_hashline_patch_file_operation("MV 'new name.txt'").expect("MV should parse"),
        Some(HashlinePatchFileOperation::Rename {
            new_path: "new name.txt".to_string(),
        })
    );
}

#[test]
fn file_operation_parser_rejects_line_ops_in_same_section() {
    let error = parse_hashline_patch_file_operation("REM\nSWAP 1:\n+omega")
        .expect_err("file op and line op should not combine");

    assert_eq!(
        error.to_string(),
        "Hashline file operations REM and MV cannot be combined with line operations in the same file section"
    );
}

#[test]
fn generated_mixed_file_patch_uses_one_apply_patch_envelope() {
    let mutations = [
        HashlinePatchFileMutation::Update(HashlinePatchFileUpdate {
            path: "a.txt",
            old_contents: "alpha\nbeta\n",
            new_contents: "alpha\nbravo\n",
            create: false,
        }),
        HashlinePatchFileMutation::Remove { path: "b.txt" },
        HashlinePatchFileMutation::Rename {
            path: "c.txt",
            new_path: "d.txt",
            contents: "move me\n",
        },
    ];
    let patch =
        apply_patch_for_hashline_mutations(&mutations, /*environment_id*/ Some("env-1"))
            .expect("mixed file patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Environment ID: env-1\n*** Update File: a.txt\n@@\n alpha\n-beta\n+bravo\n*** Delete File: b.txt\n*** Update File: c.txt\n*** Move to: d.txt\n*** End Patch"
    );
}

#[test]
fn generated_remove_patch_uses_delete_file() {
    let patch = apply_patch_for_hashline_remove("old.txt", /*environment_id*/ Some("env-1"));

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Environment ID: env-1\n*** Delete File: old.txt\n*** End Patch"
    );
}

#[test]
fn generated_rename_patch_uses_move_hunk() {
    let patch = apply_patch_for_hashline_rename(
        "old.txt",
        "new.txt",
        "first\nsecond\n",
        /*environment_id*/ None,
    );

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n*** End Patch"
    );
}

#[test]
fn find_block_prefers_smallest_brace_block() {
    let lines = [
        "fn outer() {",
        "    let value = 1;",
        "    if value > 0 {",
        "        println!(\"{value}\");",
        "    }",
        "    println!(\"done\");",
        "}",
    ];

    assert_eq!(find_block_span("src/main.rs", &lines, 4), (3, 5));
    assert_eq!(find_block_span("src/main.rs", &lines, 6), (1, 7));
}

#[test]
fn find_block_anchor_accepts_block_prefix_and_unique_hash() {
    let lines = vec!["alpha", "beta", "gamma"];

    assert_eq!(
        resolve_find_block_anchor("block 2:", &lines)
            .expect("block-prefixed anchor should resolve"),
        2
    );
    assert_eq!(
        resolve_find_block_anchor(&line_hash("gamma"), &lines)
            .expect("unique short hash should resolve"),
        3
    );
}

#[test]
fn find_block_anchor_rejects_ambiguous_short_hash() {
    let lines = vec!["same", "same"];
    let error = resolve_find_block_anchor(&line_hash("same"), &lines)
        .expect_err("ambiguous short hash should reject");

    assert!(
        error.to_string().contains("is ambiguous"),
        "unexpected error: {error}"
    );
}

#[test]
fn find_block_uses_markdown_sections() {
    let lines = [
        "# Title",
        "",
        "intro",
        "## Child",
        "body",
        "## Next",
        "next body",
    ];

    assert_eq!(find_block_span("notes.md", &lines, 3), (1, 7));
    assert_eq!(find_block_span("notes.md", &lines, 5), (4, 5));
}

#[test]
fn find_block_python_header_stops_at_next_top_level() {
    let lines = [
        "def hello():",
        "    x = 1",
        "    return x",
        "",
        "def keep():",
        "    return 2",
    ];

    assert_eq!(find_block_span("src/main.py", &lines, 1), (1, 3));
}

#[test]
fn find_block_ruby_uses_end_pairs() {
    let lines = [
        "def hello",
        "  x = 1",
        "  if true",
        "    puts 'ok'",
        "  end",
        "end",
    ];

    assert_eq!(find_block_span("src/main.rb", &lines, 2), (1, 6));
    assert_eq!(find_block_span("src/main.rb", &lines, 4), (3, 5));
}

#[test]
fn find_block_language_guess_matches_reference_extensions() {
    assert_eq!(language_for_path("src/main.rs"), "Rust");
    assert_eq!(language_for_path("src/main.py"), "Python");
    assert_eq!(language_for_path("src/main.go"), "Go");
    assert_eq!(language_for_path("include/value.hpp"), "C++");
    assert_eq!(language_for_path("notes.md"), "Markdown");
    assert_eq!(language_for_path("Makefile"), "Unknown");
}
