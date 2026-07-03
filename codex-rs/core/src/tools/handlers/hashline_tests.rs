use super::build_hashline_patch_success_body;
use super::build_hashline_read_body;
use super::hashline_block::find_block_span;
use super::hashline_block::language_for_path;
use super::hashline_hash::hash_hex;
use super::hashline_hash::line_hash;
use super::hashline_patch::apply_hashline_patch;
use super::hashline_patch::apply_patch_for_hashline_remove;
use super::hashline_patch::apply_patch_for_hashline_rename;
use super::hashline_patch::apply_patch_for_hashline_update;
use super::hashline_patch::build_hashline_patch_preview;
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
            "header": format!("[notes.txt#{}]", hash_hex(contents, 4)),
            "start_line": 1,
            "end_line": 2,
            "total_lines": 2,
            "truncated": false,
            "next_start_line": null,
            "content": format!("1:{}|alpha\n2:{}|beta", line_hash("alpha"), line_hash("beta")),
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
fn patch_rejects_multi_file_sections_with_clear_message() {
    let original = "alpha\nbeta\n";
    let patch = format!(
        "[notes.txt#{}]\nDEL 2\n[other.txt#0000]\nDEL 1",
        hash_hex(original, 4)
    );

    let error = apply_hashline_patch("notes.txt", original, &patch)
        .expect_err("multi-file sections should be rejected");

    assert!(
        error.to_string().contains("multi-file Hashline patches"),
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
    )
    .expect("rename patch should be generated");

    assert_eq!(
        patch,
        "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n first\n*** End Patch"
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
fn find_block_language_guess_matches_reference_extensions() {
    assert_eq!(language_for_path("src/main.rs"), "Rust");
    assert_eq!(language_for_path("src/main.py"), "Python");
    assert_eq!(language_for_path("src/main.go"), "Go");
    assert_eq!(language_for_path("include/value.hpp"), "C++");
    assert_eq!(language_for_path("notes.md"), "Markdown");
    assert_eq!(language_for_path("Makefile"), "Unknown");
}
