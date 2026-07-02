use pretty_assertions::assert_eq;

use super::CodeModeExecuteOutput;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;

#[test]
fn execute_output_reports_disclosed_cell_id() {
    let output = CodeModeExecuteOutput {
        output: FunctionToolOutput::from_text("Script running".to_string(), Some(true)),
        disclosed_cell_id: Some("cell-1".to_string()),
    };

    assert_eq!(
        output.disclosed_code_mode_cell_id(),
        Some("cell-1".to_string())
    );
}

#[test]
fn execute_output_without_yield_reports_no_disclosed_cell_id() {
    let output = CodeModeExecuteOutput {
        output: FunctionToolOutput::from_text("Script completed".to_string(), Some(true)),
        disclosed_cell_id: None,
    };

    assert_eq!(output.disclosed_code_mode_cell_id(), None);
}
