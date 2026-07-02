use pretty_assertions::assert_eq;

use super::ExecWaitArgs;
use super::parse_arguments;

#[test]
fn wait_args_accept_max_output_tokens_alias() {
    assert_eq!(
        parse_arguments::<ExecWaitArgs>(
            r#"{"cell_id":"cell-1","yield_time_ms":250,"max_output_tokens":42}"#
        )
        .expect("wait args should parse"),
        ExecWaitArgs {
            cell_id: "cell-1".to_string(),
            yield_time_ms: 250,
            max_tokens: Some(42),
            terminate: false,
        }
    );
}

#[test]
fn wait_args_reject_unknown_fields() {
    let error = parse_arguments::<ExecWaitArgs>(r#"{"cell_id":"cell-1","max_token":42}"#)
        .expect_err("unknown fields should be rejected")
        .to_string();

    assert!(
        error.contains("unknown field `max_token`"),
        "unexpected error: {error}"
    );
}
