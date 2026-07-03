use super::hashline_block::find_block_span;
use super::hashline_hash::hash_hex;
use super::hashline_hash::line_hash;
use super::hashline_patch::apply_hashline_patch;
use super::hashline_patch::apply_patch_for_hashline_remove;
use super::hashline_patch::apply_patch_for_hashline_rename;
use super::hashline_patch::apply_patch_for_hashline_update;
use super::hashline_patch::build_hashline_patch_preview;
use pretty_assertions::assert_eq;

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
