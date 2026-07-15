use crate::JournalError;
use crate::JournalMutation;
use crate::JournalState;
use crate::MutationProgress;
use crate::RecoveryTarget;

pub(crate) fn valid_transition(current: JournalState, next: JournalState) -> bool {
    matches!(
        (current, next),
        (JournalState::Preparing, JournalState::Prepared)
            | (JournalState::Preparing, JournalState::RollingBack)
            | (JournalState::Preparing, JournalState::RecoveryRequired)
            | (JournalState::Prepared, JournalState::Committing)
            | (JournalState::Prepared, JournalState::RollingBack)
            | (JournalState::Prepared, JournalState::RecoveryRequired)
            | (JournalState::Committing, JournalState::Committed)
            | (JournalState::Committing, JournalState::RollingBack)
            | (JournalState::Committing, JournalState::RecoveryRequired)
            | (JournalState::Committed, JournalState::Cleaning)
            | (JournalState::Committed, JournalState::RecoveryRequired)
            | (JournalState::RollingBack, JournalState::RolledBack)
            | (JournalState::RollingBack, JournalState::RecoveryRequired)
            | (JournalState::RolledBack, JournalState::Cleaning)
            | (JournalState::RolledBack, JournalState::RecoveryRequired)
            | (JournalState::Cleaning, JournalState::Complete)
            | (JournalState::Cleaning, JournalState::RecoveryRequired)
            | (JournalState::Complete, JournalState::RecoveryRequired)
            | (JournalState::RecoveryRequired, JournalState::RollingBack)
            | (JournalState::RecoveryRequired, JournalState::Cleaning)
    )
}

pub(crate) fn validate_progress(
    state: JournalState,
    mutations: &[JournalMutation],
) -> Result<(), JournalError> {
    let consistent = match state {
        JournalState::Preparing | JournalState::Prepared => mutations
            .iter()
            .all(|mutation| mutation.progress == MutationProgress::Pending),
        JournalState::Committing => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending
                    | MutationProgress::Committing
                    | MutationProgress::Applied
            )
        }),
        JournalState::Committed => mutations
            .iter()
            .all(|mutation| mutation.progress == MutationProgress::Applied),
        JournalState::RollingBack => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending
                    | MutationProgress::Committing
                    | MutationProgress::Applied
                    | MutationProgress::RollingBack
                    | MutationProgress::RolledBack
            )
        }),
        JournalState::RolledBack => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending | MutationProgress::RolledBack
            )
        }),
        JournalState::Cleaning | JournalState::Complete => {
            let all_applied = mutations
                .iter()
                .all(|mutation| mutation.progress == MutationProgress::Applied);
            let all_rolled_back = mutations.iter().all(|mutation| {
                matches!(
                    mutation.progress,
                    MutationProgress::Pending | MutationProgress::RolledBack
                )
            });
            all_applied || all_rolled_back
        }
        JournalState::RecoveryRequired => true,
    };
    if consistent {
        Ok(())
    } else {
        Err(JournalError::InconsistentProgress { state })
    }
}

pub(crate) fn validate_recovery_target(
    state: JournalState,
    target: RecoveryTarget,
    mutations: &[JournalMutation],
) -> Result<(), JournalError> {
    let consistent = match state {
        JournalState::Preparing
        | JournalState::Prepared
        | JournalState::Committing
        | JournalState::RollingBack
        | JournalState::RolledBack => target == RecoveryTarget::Rollback,
        JournalState::Committed => target == RecoveryTarget::Commit,
        JournalState::Cleaning | JournalState::Complete => match target {
            RecoveryTarget::Commit => mutations
                .iter()
                .all(|mutation| mutation.progress == MutationProgress::Applied),
            RecoveryTarget::Rollback => mutations.iter().all(|mutation| {
                matches!(
                    mutation.progress,
                    MutationProgress::Pending | MutationProgress::RolledBack
                )
            }),
        },
        JournalState::RecoveryRequired => true,
    };
    if consistent {
        Ok(())
    } else {
        Err(JournalError::InconsistentRecoveryTarget { state, target })
    }
}

pub(crate) fn valid_progress_transition(
    state: JournalState,
    current: MutationProgress,
    next: MutationProgress,
) -> bool {
    match state {
        JournalState::Committing => matches!(
            (current, next),
            (MutationProgress::Pending, MutationProgress::Committing)
                | (MutationProgress::Committing, MutationProgress::Applied)
        ),
        JournalState::RollingBack => matches!(
            (current, next),
            (MutationProgress::Applied, MutationProgress::RollingBack)
                | (MutationProgress::Committing, MutationProgress::RollingBack)
                | (MutationProgress::RollingBack, MutationProgress::RolledBack)
        ),
        _ => false,
    }
}
