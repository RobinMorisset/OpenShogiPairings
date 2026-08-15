//! The one way a serialized tournament becomes a [`Tournament`].
//!
//! Every path that reads an untrusted save — server startup, a backup restore,
//! `POST /api/tournaments/import` — goes through [`load`], which does the same
//! three things in the same order:
//!
//! 1. **Read the format version first**, before anything tries to parse the
//!    tournament itself. A format bump can reshape a field's serde
//!    representation (v5 made `handicap_policy` an internally-tagged enum, so an
//!    old bare-string value stopped deserializing at all), so a full parse of an
//!    old file fails deep inside a changed field with an opaque error naming
//!    anything but the real cause — the version, which the version field exists
//!    precisely to surface.
//! 2. **Upgrade a save this build still supports** ([`UPGRADABLE_FROM`]), or
//!    reject the version outright.
//! 3. **Parse, then [`Tournament::validate_loaded`]**. Parsing proves the shape,
//!    not the invariants, and the read path trusts both: a bracket that
//!    disagrees with its cup size panics when the first `GET` derives it.
//!
//! Lives in the server (not `osp-core`) because it is the server that parses
//! JSON — core has no runtime JSON dependency.

use osp_core::{CupFormat, Tournament, TournamentError, TOURNAMENT_FORMAT_VERSION};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The one older format version this build still reads: what OpenShogiPairings
/// v1.1.0, v1.2.0 and v1.3.0 all wrote.
///
/// Those three releases share this format — v1.2.0 added player and tournament
/// `categories` without a bump, because the field defaults, so a v1.1.0 save is
/// a strict subset of a v1.3.0 one and both land here. Only v1.0.0 (format 4) is
/// out of reach.
///
/// What is deliberately narrow is *which tournaments* are upgraded: only the
/// **not-yet-started** ones, a list of players and settings whose fields turn out
/// to be a pure subset of today's (see [`upgrade_from_v5`]). Rounds are where the
/// seven intervening bumps actually bite (boards gained an outcome sum in v6,
/// forfeit reasons in v8, frozen pairing explanations in v10, one record per
/// round of a long game in v11, and the handicap moved inside the outcome in
/// v12), and a referee does not upgrade the binary mid-event — so a started v5
/// save is refused with [`TournamentError::OldSaveAlreadyStarted`] rather than
/// half-converted.
///
/// Versions 6..11 were never released and are rejected like any other unknown
/// version.
pub(crate) const UPGRADABLE_FROM: u32 = 5;

/// Parse a serialized tournament, upgrading it if it is an
/// [`UPGRADABLE_FROM`] save, and hold it to the same invariants a tournament
/// this build built itself must satisfy.
///
/// This is the *only* gate between an untrusted file and the registry.
pub(crate) fn load(bytes: &[u8]) -> Result<Tournament, TournamentError> {
    let tournament = match probe_version(bytes)? {
        TOURNAMENT_FORMAT_VERSION => parse(bytes)?,
        UPGRADABLE_FROM => parse_upgraded(bytes)?,
        found => {
            return Err(TournamentError::UnsupportedFormatVersion {
                found,
                supported: TOURNAMENT_FORMAT_VERSION,
            })
        }
    };
    tournament.validate_loaded()?;
    Ok(tournament)
}

/// Peek at only the `format_version`, so the version can be judged before the
/// full parse (reason 1 in the module docs).
fn probe_version(bytes: &[u8]) -> Result<u32, TournamentError> {
    #[derive(Deserialize)]
    struct VersionProbe {
        // Required, matching the `Tournament` field itself: a save with no
        // version is one whose shape nothing here can vouch for, and calling it
        // "current" would be a guess dressed up as a fact.
        format_version: u32,
    }
    let probe: VersionProbe = serde_json::from_slice(bytes).map_err(malformed)?;
    Ok(probe.format_version)
}

fn parse(bytes: &[u8]) -> Result<Tournament, TournamentError> {
    serde_json::from_slice(bytes).map_err(malformed)
}

fn parse_upgraded(bytes: &[u8]) -> Result<Tournament, TournamentError> {
    let mut save: Value = serde_json::from_slice(bytes).map_err(malformed)?;
    upgrade_from_v5(&mut save)?;
    serde_json::from_value(save).map_err(malformed)
}

fn malformed(e: serde_json::Error) -> TournamentError {
    TournamentError::MalformedSave(e.to_string())
}

/// Bring a v5 (v1.3.0) save of a **not-yet-started** tournament up to the
/// current format, in place.
///
/// Everything a v5 save says about its players and settings is still said the
/// same way today: the five bumps since only *added* fields, all of which carry
/// a serde default, so the referee's floater style, MacMahon thresholds,
/// handicap policy, categories and point adjustments all survive untouched. Only
/// two added fields have no default, both by deliberate choice at the time —
/// `Tournament::explanations_faithful_through` (v10) and `Cup::format` (the
/// qualifier format) — so those are all this has to supply.
///
/// Refuses a tournament that is under way, for the reason on [`UPGRADABLE_FROM`].
/// A pending `draft` counts as under way: its `forced_boards` are v5-shaped
/// `Board`s, and `Board` does not deny unknown fields, so an old board's
/// `result`/`drawn`/`no_show` would be dropped *silently* rather than refused.
/// Losing a round-in-preparation is thirty seconds of the referee's time;
/// silently losing what it said is the failure worth avoiding.
fn upgrade_from_v5(save: &mut Value) -> Result<(), TournamentError> {
    let save = object_mut(save, "the save")?;

    let started = match save.get("rounds") {
        None | Some(Value::Null) => false,
        Some(Value::Array(rounds)) => !rounds.is_empty(),
        Some(_) => {
            return Err(TournamentError::MalformedSave(
                "`rounds` is not a list".into(),
            ))
        }
    } || !matches!(save.get("draft"), None | Some(Value::Null));
    if started {
        return Err(TournamentError::OldSaveAlreadyStarted {
            found: UPGRADABLE_FROM,
            supported: TOURNAMENT_FORMAT_VERSION,
        });
    }

    save.insert(
        "format_version".into(),
        Value::from(TOURNAMENT_FORMAT_VERSION),
    );
    // v10. `0` — "no round's explanation is faithful" — is the only honest value
    // for a save that predates explanations, and it is what a tournament with no
    // rounds carries anyway.
    save.insert("explanations_faithful_through".into(), Value::from(0));
    // A tournament finalized with a cup, but not yet started, already has its
    // bracket frozen. v5 had only the direct format, so name it from the enum
    // rather than a string literal — a renamed variant then breaks the build
    // here instead of writing a value that no longer deserializes.
    if let Some(cup) = save.get_mut("cup").filter(|cup| !cup.is_null()) {
        let cup = object_mut(cup, "`cup`")?;
        cup.insert(
            "format".into(),
            serde_json::to_value(CupFormat::Direct).expect("a fieldless enum serializes"),
        );
    }
    Ok(())
}

fn object_mut<'a>(
    value: &'a mut Value,
    what: &str,
) -> Result<&'a mut Map<String, Value>, TournamentError> {
    value
        .as_object_mut()
        .ok_or_else(|| TournamentError::MalformedSave(format!("{what} is not a JSON object")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real v1.3.0 save, written by v1.3.0's own serializer: a not-yet-started
    /// tournament with every settings knob set away from its default, so this
    /// pins the whole v5 settings schema and not just the easy parts.
    const V5_SETTINGS: &str = include_str!("../tests/fixtures/v1.3.0-settings.json");
    /// A real v1.3.0 save whose registration is finalized *with a cup* — still
    /// not started, and the only shape that carries the second undefaulted
    /// field, `Cup::format`.
    const V5_CUP: &str = include_str!("../tests/fixtures/v1.3.0-cup.json");
    /// The same tournament as `V5_SETTINGS` with one round played: the shape the
    /// upgrade must refuse rather than half-convert.
    const V5_STARTED: &str = include_str!("../tests/fixtures/v1.3.0-started.json");
    /// The *same* format written by v1.1.0, the oldest release that wrote v5 —
    /// so it lacks the two `categories` lists v1.2.0 added without a bump. It is
    /// the fewest-keys v5 save that exists, which is what makes it the one that
    /// catches a field added here without a serde default.
    const V5_OLDEST: &str = include_str!("../tests/fixtures/v1.1.0-settings.json");

    #[test]
    fn a_current_save_round_trips() {
        let bytes = serde_json::to_vec(&Tournament::new("Cup").unwrap()).unwrap();
        assert_eq!(load(&bytes).unwrap().name, "Cup");
    }

    #[test]
    fn a_v5_save_keeps_every_setting_the_referee_chose() {
        let t = load(V5_SETTINGS.as_bytes()).expect("a v1.3.0 save must still open");
        assert_eq!(t.format_version, TOURNAMENT_FORMAT_VERSION);
        assert_eq!(t.name, "Maximal v1.3.0 Open");
        assert_eq!(t.explanations_faithful_through, 0);

        // Settings the referee set, all the way down the nested enums.
        let s = &t.settings;
        assert!(s.cup_enabled && s.long_boards_enabled && s.half_point_absences);
        assert_eq!(s.floater_style(), osp_core::FloaterStyle::Median);
        assert_eq!(s.macmahon_thresholds().len(), 2);
        assert_eq!(s.categories.len(), 1);
        assert_eq!(s.categories[0].name, "Juniors");
        // Everything added since v5 takes its default, which is what makes the
        // upgrade a no-op beyond the two undefaulted fields.
        assert_eq!(s.cup_format, CupFormat::Direct);
        assert!(s.teams.is_none());
        assert_eq!(s.city, None);

        // Players, including the fields that were themselves new in v4/v5.
        assert_eq!(t.players.len(), 2);
        assert_eq!(t.players[0].rating, Some(1850));
        assert_eq!(t.players[0].club.as_deref(), Some("Nancy"));
        assert_eq!(t.players[0].adjustments.len(), 1);
        assert_eq!(t.players[0].categories.len(), 1);
        assert_eq!(t.players[0].pairing_rating, None);
    }

    /// v1.1.0 and v1.2.0 wrote this same format 5, so their saves open here too —
    /// the version is the contract, not the release that happened to write it.
    #[test]
    fn the_oldest_v5_save_opens_with_the_keys_it_never_had_defaulted() {
        let t = load(V5_OLDEST.as_bytes()).expect("a v1.1.0 save is still a v5 save");
        assert_eq!(t.format_version, TOURNAMENT_FORMAT_VERSION);
        assert_eq!(t.name, "Maximal v1.1.0 Open");
        assert_eq!(t.players.len(), 2);
        // Set in v1.1.0 and still meant the same thing.
        assert_eq!(t.settings.floater_style(), osp_core::FloaterStyle::Median);
        assert_eq!(t.players[0].rating, Some(1850));
        assert_eq!(t.players[0].adjustments.len(), 1);
        // Added in v1.2.0 without a format bump, so this file predates them and
        // they have to default rather than fail the parse.
        assert!(t.settings.categories.is_empty());
        assert!(t.players[0].categories.is_empty());
    }

    #[test]
    fn a_v5_save_finalized_with_a_cup_gets_the_direct_format() {
        let t = load(V5_CUP.as_bytes()).expect("a finalized-but-unstarted save must still open");
        let cup = t.cup.as_ref().expect("it was finalized with a cup");
        assert_eq!(cup.size, 8);
        // v5 had no qualifier format, so `direct` is the only truthful answer.
        assert_eq!(cup.format, CupFormat::Direct);
        assert_eq!(cup.seed_order.len(), 8);
    }

    #[test]
    fn a_v5_save_of_a_started_tournament_is_refused() {
        assert!(matches!(
            load(V5_STARTED.as_bytes()),
            Err(TournamentError::OldSaveAlreadyStarted {
                found: 5,
                supported: TOURNAMENT_FORMAT_VERSION
            })
        ));
    }

    #[test]
    fn a_v5_save_with_a_round_in_preparation_is_refused() {
        let mut save: Value = serde_json::from_str(V5_SETTINGS).unwrap();
        save["draft"] = serde_json::json!({ "number": 1, "absent": [] });
        let bytes = serde_json::to_vec(&save).unwrap();
        assert!(matches!(
            load(&bytes),
            Err(TournamentError::OldSaveAlreadyStarted { .. })
        ));
    }

    #[test]
    fn versions_outside_the_window_are_rejected_by_version() {
        // The v1.0.0 files were format 4 — too old to upgrade. This is the case a
        // full deserialize would instead fail on with an opaque field-level error.
        let old = br#"{"format_version":4,"handicap_policy":"allowed"}"#;
        assert!(matches!(
            load(old),
            Err(TournamentError::UnsupportedFormatVersion { found: 4, .. })
        ));
        // 6..9 were never released, and a version from the future can only come
        // from a newer build.
        for found in [6, 7, 8, 9, 999] {
            let bytes = format!(r#"{{"format_version":{found},"name":"X"}}"#);
            assert!(
                matches!(
                    load(bytes.as_bytes()),
                    Err(TournamentError::UnsupportedFormatVersion { .. })
                ),
                "format {found} must be rejected on its version"
            );
        }
    }

    #[test]
    fn a_save_with_no_version_at_all_is_refused() {
        // Not treated as current: a file that doesn't say what shape it is in is
        // one nothing here can vouch for, and the message says which field is
        // missing rather than failing later on some unrelated field.
        let Err(TournamentError::MalformedSave(details)) = load(b"{}") else {
            panic!("a save with no format_version must be refused");
        };
        assert!(
            details.contains("format_version"),
            "the error should name the missing field: {details}"
        );
    }

    #[test]
    fn bytes_that_are_not_json_are_a_malformed_save() {
        assert!(matches!(
            load(b"not json"),
            Err(TournamentError::MalformedSave(_))
        ));
    }

    #[test]
    fn invariants_are_checked_after_the_upgrade_too() {
        // `validate_loaded` runs on the upgraded record, so a v5 file that parses
        // but breaks an invariant is still refused.
        let mut save: Value = serde_json::from_str(V5_SETTINGS).unwrap();
        save["name"] = Value::from("   ");
        let bytes = serde_json::to_vec(&save).unwrap();
        assert!(matches!(
            load(&bytes),
            Err(TournamentError::EmptyTournamentName)
        ));
    }
}
