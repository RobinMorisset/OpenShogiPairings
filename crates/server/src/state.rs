//! Shared server state.

use std::sync::{Arc, RwLock};

use osp_core::{Tournament, TournamentError};

use crate::ratings::CachedRatings;

/// The current tournament plus a linear undo history.
///
/// The history is a stack of full tournament snapshots taken *before* each
/// player mutation; undo pops the most recent one. Snapshots are cheap (a
/// tournament is just a list of players), which is why we snapshot rather than
/// track inverse commands. Creating or loading a tournament resets the history,
/// so undo is scoped to edits of the current tournament.
#[derive(Default)]
pub struct TournamentStore {
    current: Option<Tournament>,
    history: Vec<Tournament>,
}

impl TournamentStore {
    /// The current tournament, if one exists.
    pub fn current(&self) -> Option<&Tournament> {
        self.current.as_ref()
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Set the current tournament and clear the undo history.
    ///
    /// Used by create and load — these establish a new baseline, so prior
    /// history no longer applies.
    pub fn set_current(&mut self, tournament: Tournament) {
        self.current = Some(tournament);
        self.history.clear();
    }

    /// Apply a mutation to the current tournament, checkpointing first.
    ///
    /// The mutation runs against a clone; only if it succeeds do we snapshot the
    /// previous state onto the history and swap in the new one. A failed
    /// mutation (e.g. validation error) leaves state and history untouched.
    pub fn mutate<F>(&mut self, f: F) -> Result<(), MutateError>
    where
        F: FnOnce(&mut Tournament) -> Result<(), TournamentError>,
    {
        let mut next = match &self.current {
            Some(current) => current.clone(),
            None => return Err(MutateError::NoTournament),
        };
        f(&mut next).map_err(MutateError::Domain)?;
        let previous = self.current.replace(next).expect("current was Some");
        self.history.push(previous);
        Ok(())
    }

    /// Revert to the previous state, if any. No-op when history is empty.
    pub fn undo(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.current = Some(previous);
        }
    }
}

/// Failure of [`TournamentStore::mutate`].
pub enum MutateError {
    /// No tournament exists to mutate.
    NoTournament,
    /// The mutation itself violated a domain rule.
    Domain(TournamentError),
}

/// State shared across all requests.
///
/// The tournament store is the single source of truth every connected referee
/// reads and mutates; it and the ratings cache are `RwLock`-guarded and
/// `Arc`-shared so axum can clone the state cheaply per request.
#[derive(Clone, Default)]
pub struct AppState {
    pub store: Arc<RwLock<TournamentStore>>,
    /// Cached FESA rating list (see [`crate::ratings`]).
    pub ratings: Arc<RwLock<Option<CachedRatings>>>,
}
