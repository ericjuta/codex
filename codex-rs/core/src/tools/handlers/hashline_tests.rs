use super::apply_hashline_patch;
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
