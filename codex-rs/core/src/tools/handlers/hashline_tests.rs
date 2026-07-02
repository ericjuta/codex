use super::apply_hashline_patch;
use super::apply_patch_for_hashline_remove;
use super::apply_patch_for_hashline_rename;
use super::apply_patch_for_hashline_update;
use super::line_hash;
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

    let updated = apply_hashline_patch(original, &patch).expect("patch should apply");

    assert_eq!(updated, "bravo\ngamma\ndelta\n");
}

#[test]
fn rejects_stale_line_hash() {
    let error = apply_hashline_patch("alpha\n", "SWAP 1:00|omega")
        .expect_err("stale hash should be rejected");

    assert!(
        error.to_string().contains("hash mismatch"),
        "unexpected error: {error}"
    );
}

#[test]
fn accepts_compact_head_and_tail_insert_syntax() {
    let updated = apply_hashline_patch("middle\n", "INS.HEAD|top\nINS.TAIL|bottom")
        .expect("compact insert syntax should apply");

    assert_eq!(updated, "top\nmiddle\nbottom\n");
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
