//! Naming the change an undo would revert.
//!
//! The undo history is a stack of snapshots (see [`crate::state::TournamentStore`]),
//! which is enough to *perform* an undo but says nothing about what it would
//! take back. That is fine for one referee working alone — the last thing they
//! did is the thing they remember doing — and useless the moment two of them
//! share a tournament, where the button reverts somebody else's work and the
//! only honest thing it can do is say whose and what.
//!
//! So every mutation carries a label onto the stack, and the current top of it
//! rides along in every [`TournamentView`](crate::tournament::TournamentView).
//! The label is a stable machine [`ChangeCode`] plus language-neutral
//! interpolation `values`, the same shape errors and blocked reasons already
//! travel in: the wording is the client's business, the decision about *what
//! happened* is the server's — the frontend cannot re-derive it, because by the
//! time it asks, the change has already been folded into the tournament.
//!
//! A code is part of the API contract. `keyUsage.test.ts` expands [`ChangeCode`]'s
//! generated TypeScript union into the `undo.*` keys the catalogues must define,
//! so a variant added here without nine translations fails the frontend build
//! naming itself.

use std::collections::BTreeMap;

use osp_core::{NewPlayer, Tournament};
use serde::Serialize;
use uuid::Uuid;

/// What a change did, in the shape the client renders: a stable machine code and
/// its interpolation values.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct ChangeLabel {
    pub(crate) code: ChangeCode,
    /// Language-neutral interpolation values, empty for a label that needs none
    /// — always present on the wire, like [`BlockedReason`](crate::tournament)'s,
    /// so the client has one shape to render rather than two.
    pub(crate) values: BTreeMap<String, String>,
}

/// The kind of change an undo would revert.
///
/// Deliberately coarser than the endpoint list: several routes that a referee
/// would describe the same way share a code, because the label answers "what am
/// I about to take back", not "which handler ran". Editing a player's rating,
/// their cup eligibility and their category are all [`EditPlayer`](Self::EditPlayer);
/// a winner, a draw, a no-show and a long-game flag are all
/// [`BoardResult`](Self::BoardResult).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[ts(rename_all = "snake_case")]
pub(crate) enum ChangeCode {
    Settings,
    /// The last round (or the open draft) was cancelled.
    CancelRound,
    /// A round was opened for drafting.
    PrepareRound,
    /// The open draft was edited (absences, forced pairings, forced byes).
    EditDraft,
    /// A draft was confirmed and its round started.
    StartRound,
    /// A round was re-paired around a forced pairing.
    ForcePairing,
    /// A board's outcome: a winner, a draw, a no-show, or a long-game flag.
    BoardResult,
    BoardHandicap,
    /// What a round scored somebody who sat it out.
    SitoutValue,
    RegisterPlayer,
    /// A roster was imported from a CSV file.
    ImportPlayers,
    /// A registered player's details changed.
    EditPlayer,
    /// Cup eligibility changed for a batch of players at once.
    ///
    /// Deliberately uncounted: a counted noun needs the plural arms of nine
    /// languages behind it, and "how many" is not what the referee is about to
    /// take back — the one instruction they gave is.
    EditPlayers,
    /// A player was removed from the tournament.
    RemovePlayer,
    /// A manual point bonus/malus was added or removed.
    AdjustPoints,
    AddTeam,
    RenameTeam,
    RemoveTeam,
    /// A team's roster or board order changed.
    EditTeam,
}

impl ChangeLabel {
    /// A label with no interpolation values.
    fn bare(code: ChangeCode) -> Self {
        Self {
            code,
            values: BTreeMap::new(),
        }
    }

    /// A label carrying interpolation values. Values must be language-neutral
    /// (a name, a number); the catalogues put them into the translated phrase.
    fn with(code: ChangeCode, values: impl IntoIterator<Item = (&'static str, String)>) -> Self {
        Self {
            code,
            values: values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    pub(crate) fn settings() -> Self {
        Self::bare(ChangeCode::Settings)
    }

    pub(crate) fn cancel_round() -> Self {
        Self::bare(ChangeCode::CancelRound)
    }

    pub(crate) fn prepare_round(round: u32) -> Self {
        Self::with(ChangeCode::PrepareRound, [("round", round.to_string())])
    }

    pub(crate) fn edit_draft(round: u32) -> Self {
        Self::with(ChangeCode::EditDraft, [("round", round.to_string())])
    }

    pub(crate) fn start_round(round: u32) -> Self {
        Self::with(ChangeCode::StartRound, [("round", round.to_string())])
    }

    pub(crate) fn force_pairing(round: u32) -> Self {
        Self::with(ChangeCode::ForcePairing, [("round", round.to_string())])
    }

    pub(crate) fn board_result(round: u32, board_index: usize) -> Self {
        Self::board(ChangeCode::BoardResult, round, board_index)
    }

    pub(crate) fn board_handicap(round: u32, board_index: usize) -> Self {
        Self::board(ChangeCode::BoardHandicap, round, board_index)
    }

    /// The two board-level labels, which name the board the way the round view
    /// does: numbered from 1, not indexed from 0.
    fn board(code: ChangeCode, round: u32, board_index: usize) -> Self {
        Self::with(
            code,
            [
                ("round", round.to_string()),
                ("board", (board_index + 1).to_string()),
            ],
        )
    }

    pub(crate) fn sitout_value(round: u32) -> Self {
        Self::with(ChangeCode::SitoutValue, [("round", round.to_string())])
    }

    /// Named from the request rather than from the tournament, since the player
    /// this is about does not exist there yet.
    pub(crate) fn register_player(new: &NewPlayer) -> Self {
        Self::named(
            ChangeCode::RegisterPlayer,
            format!(
                "{} {}",
                new.last_name,
                new.first_name.as_deref().unwrap_or("")
            )
            .trim()
            .to_string(),
        )
    }

    /// Deliberately unnumbered. The file's row count is the obvious thing to
    /// interpolate and would be a lie: an import registers only the rows naming
    /// somebody not already in the tournament, and how many that was is known
    /// after the mutation, not before it — where the label has to be built.
    pub(crate) fn import_players() -> Self {
        Self::bare(ChangeCode::ImportPlayers)
    }

    pub(crate) fn edit_player(tournament: Option<&Tournament>, player: Uuid) -> Self {
        Self::named(ChangeCode::EditPlayer, player_name(tournament, player))
    }

    pub(crate) fn edit_players() -> Self {
        Self::bare(ChangeCode::EditPlayers)
    }

    pub(crate) fn remove_player(tournament: Option<&Tournament>, player: Uuid) -> Self {
        Self::named(ChangeCode::RemovePlayer, player_name(tournament, player))
    }

    /// The point adjustment label, for a player or a whole team — the referee
    /// reads "the point adjustment for X" the same way either way, so the two
    /// share a code and differ only in whose name is interpolated.
    pub(crate) fn adjust_points(name: String) -> Self {
        Self::named(ChangeCode::AdjustPoints, name)
    }

    pub(crate) fn add_team(name: &str) -> Self {
        Self::named(ChangeCode::AddTeam, name.to_string())
    }

    pub(crate) fn rename_team(tournament: Option<&Tournament>, team: Uuid) -> Self {
        Self::named(ChangeCode::RenameTeam, team_name(tournament, team))
    }

    pub(crate) fn remove_team(tournament: Option<&Tournament>, team: Uuid) -> Self {
        Self::named(ChangeCode::RemoveTeam, team_name(tournament, team))
    }

    pub(crate) fn edit_team(tournament: Option<&Tournament>, team: Uuid) -> Self {
        Self::named(ChangeCode::EditTeam, team_name(tournament, team))
    }

    fn named(code: ChangeCode, name: String) -> Self {
        Self::with(code, [("name", name)])
    }
}

/// How a player is named in a label: `Last First`, the spelling every list in
/// the app uses.
///
/// Names nobody when the id is unknown — or when there is no tournament at all
/// — rather than erroring, and that is sound rather than sloppy: the label is
/// read from the state the mutation is *about to* run against, so either case
/// means that mutation is one line away from failing (`PlayerNotFound`,
/// `NoTournament`). A failed mutation pushes nothing onto the history, so this
/// string is discarded unread.
pub(crate) fn player_name(tournament: Option<&Tournament>, player: Uuid) -> String {
    tournament
        .and_then(|t| t.players.iter().find(|p| p.id == player))
        .map(|p| {
            format!("{} {}", p.last_name, p.first_name)
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

/// The team's name, on the same terms as [`player_name`].
pub(crate) fn team_name(tournament: Option<&Tournament>, team: Uuid) -> String {
    tournament
        .and_then(|t| t.teams.iter().find(|t| t.id == team))
        .map(|t| t.name.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_is_named_from_one_like_the_round_view() {
        let label = ChangeLabel::board_result(2, 0);
        assert_eq!(label.values.get("round").map(String::as_str), Some("2"));
        assert_eq!(label.values.get("board").map(String::as_str), Some("1"));
    }

    #[test]
    fn a_player_name_is_last_then_first_and_trims_a_missing_half() {
        let mut tournament = Tournament::new("Cup").unwrap();
        let id = tournament
            .add_player(osp_core::NewPlayer {
                last_name: "Habu".into(),
                first_name: Some("Yoshiharu".into()),
                ..Default::default()
            })
            .unwrap()
            .id;
        assert_eq!(player_name(Some(&tournament), id), "Habu Yoshiharu");

        // A player with no given name must not come back with a trailing space.
        let nameless = tournament
            .add_player(osp_core::NewPlayer {
                last_name: "Alpha".into(),
                ..Default::default()
            })
            .unwrap()
            .id;
        assert_eq!(player_name(Some(&tournament), nameless), "Alpha");
    }

    /// The lookups that cannot fail loudly, because the mutation behind them is
    /// about to: an unknown id — or no tournament at all — must not panic on
    /// the way to its own 404.
    #[test]
    fn an_unknown_id_names_nobody() {
        let tournament = Tournament::new("Cup").unwrap();
        assert_eq!(player_name(Some(&tournament), Uuid::nil()), "");
        assert_eq!(team_name(Some(&tournament), Uuid::nil()), "");
        assert_eq!(player_name(None, Uuid::nil()), "");
        assert_eq!(team_name(None, Uuid::nil()), "");
    }
}
