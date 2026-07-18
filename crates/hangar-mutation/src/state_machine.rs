//! Destructive-action state machine. Pure logic — no file operations, no I/O.
//!
//! ```text
//! Draft --build--> Reviewed --confirm--> Confirmed
//! Confirmed --backup--> BackupRunning --verify--> BackupVerified
//! Confirmed | BackupVerified --validate--> Executing --> Verifying --> Done
//! any --error--> Failed
//! Failed --recover--> RolledBack | Executing (resume)
//! ```

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationState {
    Draft,
    Reviewed,
    Confirmed,
    BackupRunning,
    BackupVerified,
    Executing,
    Verifying,
    Done,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid operation transition from {from:?} to {to:?}")]
pub struct StateError {
    pub from: OperationState,
    pub to: OperationState,
}

impl OperationState {
    /// Whether `self -> to` is a permitted transition. Any state may fail.
    pub fn can_transition_to(self, to: OperationState) -> bool {
        use OperationState::*;
        if to == Failed {
            return true;
        }
        // Gate 3: a destructive Execute may only follow a verified backup. The
        // direct Confirmed -> Executing edge (delete without backup) is removed.
        matches!(
            (self, to),
            (Draft, Reviewed)
                | (Reviewed, Confirmed)
                | (Confirmed, BackupRunning)
                | (BackupRunning, BackupVerified)
                | (BackupVerified, Executing)
                | (Executing, Verifying)
                | (Verifying, Done)
                | (Failed, RolledBack)
                | (Failed, Executing)
        )
    }

    /// Apply a transition or return a `StateError` if it is not permitted.
    pub fn transition(self, to: OperationState) -> Result<OperationState, StateError> {
        if self.can_transition_to(to) {
            Ok(to)
        } else {
            Err(StateError { from: self, to })
        }
    }

    /// Terminal states never transition further (except recovery from Failed).
    pub fn is_terminal(self) -> bool {
        matches!(self, OperationState::Done | OperationState::RolledBack)
    }
}

#[cfg(test)]
mod tests {
    use super::OperationState::*;

    #[test]
    fn execute_requires_a_verified_backup_first() {
        // Gate 3: the model has no Confirmed -> Executing edge. A destructive
        // execute may only follow BackupRunning -> BackupVerified.
        assert!(Confirmed.transition(Executing).is_err());
        assert!(BackupVerified.can_transition_to(Executing));
    }

    #[test]
    fn happy_path_with_backup() {
        let mut state = Confirmed;
        for next in [BackupRunning, BackupVerified, Executing, Verifying, Done] {
            state = state.transition(next).unwrap();
        }
        assert_eq!(state, Done);
    }

    #[test]
    fn any_state_can_fail_then_roll_back_or_resume() {
        assert!(Executing.can_transition_to(Failed));
        assert!(BackupRunning.can_transition_to(Failed));
        assert!(Failed.transition(RolledBack).is_ok());
        assert!(Failed.transition(Executing).is_ok());
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        assert!(Draft.transition(Executing).is_err()); // must review + confirm first
        assert!(Reviewed.transition(Done).is_err());
        assert!(Done.transition(Executing).is_err()); // terminal
        assert!(Confirmed.transition(Verifying).is_err());
    }
}
