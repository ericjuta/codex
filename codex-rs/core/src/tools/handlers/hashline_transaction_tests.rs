use super::ExactBytesDigest;
use super::HashlineTransactionArgs;
use super::ToolTransactionAction;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn commit_previewed_action_accepts_published_name_and_legacy_alias() {
    let expected_plan_digest = ExactBytesDigest::from_array([0x5a; 32]);

    for field_name in ["expectedPlanDigest", "expected_plan_digest"] {
        let mut action = json!({ "type": "commitPreviewed" });
        action[field_name] = json!(expected_plan_digest.to_string());
        let arguments = json!({
            "action": action,
            "mutations": [],
        });

        let args = serde_json::from_value::<HashlineTransactionArgs>(arguments)
            .expect("commitPreviewed action should deserialize");
        let ToolTransactionAction::CommitPreviewed {
            expected_plan_digest: actual_plan_digest,
        } = args.action
        else {
            panic!("expected commitPreviewed action");
        };
        assert_eq!(actual_plan_digest, expected_plan_digest);
    }
}
