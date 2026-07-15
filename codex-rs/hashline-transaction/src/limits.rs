use crate::FileEdit;
use crate::FileMutation;
use crate::PlanError;
use crate::TransactionAction;
use crate::TransactionLimits;
use crate::TransactionRequest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RequestCosts {
    pub input_bytes: u64,
    pub edits: u64,
    pub edit_lines: u64,
}

pub(crate) fn request_costs(request: &TransactionRequest) -> RequestCosts {
    let mut costs = RequestCosts {
        input_bytes: request
            .environment_id
            .len()
            .saturating_add(request.root.to_string().len()) as u64,
        edits: 0,
        edit_lines: 0,
    };
    if matches!(request.action, TransactionAction::CommitPreviewed { .. }) {
        costs.input_bytes = costs.input_bytes.saturating_add(32);
    }
    for mutation in &request.mutations {
        costs.input_bytes = costs
            .input_bytes
            .saturating_add(mutation_input_bytes(mutation));
        costs.edits = costs.edits.saturating_add(mutation_edit_count(mutation));
        costs.edit_lines = costs
            .edit_lines
            .saturating_add(mutation_edit_line_count(mutation));
    }
    costs
}

pub(crate) fn validate_request_limits(
    request: &TransactionRequest,
    limits: TransactionLimits,
) -> Result<(), PlanError> {
    if request.mutations.is_empty() {
        return Err(PlanError::Empty);
    }
    check_limit(
        "mutation count",
        request.mutations.len() as u64,
        limits.max_mutations,
    )?;
    let costs = request_costs(request);
    check_limit("edit count", costs.edits, limits.max_edits)?;
    check_limit("edit line count", costs.edit_lines, limits.max_edit_lines)?;
    check_limit("input bytes", costs.input_bytes, limits.max_input_bytes)
}

pub(crate) fn check_limit(
    resource: &'static str,
    observed: u64,
    limit: u64,
) -> Result<(), PlanError> {
    if observed > limit {
        Err(PlanError::Limit {
            resource,
            observed,
            limit,
        })
    } else {
        Ok(())
    }
}

fn mutation_input_bytes(mutation: &FileMutation) -> u64 {
    match mutation {
        FileMutation::Create { path, contents } => {
            byte_len(path).saturating_add(byte_len(contents))
        }
        FileMutation::Update {
            path,
            expected: _,
            edits,
        } => byte_len(path).saturating_add(32).saturating_add(
            edits
                .iter()
                .map(edit_input_bytes)
                .fold(0, u64::saturating_add),
        ),
        FileMutation::Delete { path, expected: _ } => byte_len(path).saturating_add(32),
        FileMutation::Move {
            source,
            expected: _,
            destination,
            edits,
        } => byte_len(source)
            .saturating_add(32)
            .saturating_add(byte_len(destination))
            .saturating_add(
                edits
                    .iter()
                    .map(edit_input_bytes)
                    .fold(0, u64::saturating_add),
            ),
    }
}

fn mutation_edit_count(mutation: &FileMutation) -> u64 {
    match mutation {
        FileMutation::Create { .. } | FileMutation::Delete { .. } => 0,
        FileMutation::Update { edits, .. } | FileMutation::Move { edits, .. } => edits.len() as u64,
    }
}

fn mutation_edit_line_count(mutation: &FileMutation) -> u64 {
    match mutation {
        FileMutation::Create { .. } | FileMutation::Delete { .. } => 0,
        FileMutation::Update { edits, .. } | FileMutation::Move { edits, .. } => edits
            .iter()
            .map(|edit| match edit {
                FileEdit::ReplaceAll { .. } => 0,
                FileEdit::ReplaceLines { lines, .. }
                | FileEdit::InsertBefore { lines, .. }
                | FileEdit::InsertAfter { lines, .. } => lines.len() as u64,
            })
            .fold(0, u64::saturating_add),
    }
}

fn edit_input_bytes(edit: &FileEdit) -> u64 {
    match edit {
        FileEdit::ReplaceAll { contents } => byte_len(contents),
        FileEdit::ReplaceLines { range, lines } => anchor_bytes(&range.start)
            .saturating_add(anchor_bytes(&range.end))
            .saturating_add(lines.iter().map(byte_len).fold(0, u64::saturating_add)),
        FileEdit::InsertBefore { anchor, lines } | FileEdit::InsertAfter { anchor, lines } => {
            anchor_bytes(anchor)
                .saturating_add(lines.iter().map(byte_len).fold(0, u64::saturating_add))
        }
    }
}

fn anchor_bytes(anchor: &crate::LineAnchor) -> u64 {
    8_u64.saturating_add(byte_len(&anchor.expected_hash))
}

fn byte_len(value: impl AsRef<[u8]>) -> u64 {
    value.as_ref().len() as u64
}
