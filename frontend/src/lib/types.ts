// Shared API types.
//
// The DTO shapes are generated from the Rust `osp-core` / `osp-server` types by
// ts-rs (see crates/*/src, `#[derive(TS)]`) and live in ./generated. This module
// re-exports them under one import path and adds the few hand-maintained display
// tables that pair a generated type with UI metadata.
//
// To regenerate after changing a Rust DTO: `cargo test export_bindings`, then
// commit the ./generated diff (CI checks it is up to date).

export type { AffectedCycle } from "./generated/AffectedCycle";
export type { BackupInfo } from "./generated/BackupInfo";
export type { BackupList } from "./generated/BackupList";
export type { Board } from "./generated/Board";
export type { BracketMatch } from "./generated/BracketMatch";
export type { CupBracketView } from "./generated/CupBracketView";
export type { BoardLedger } from "./generated/BoardLedger";
export type { ClubProtection } from "./generated/ClubProtection";
export type { NationalityProtection } from "./generated/NationalityProtection";
export type { HandicapDisplay } from "./generated/HandicapDisplay";
export type { Counterfactual } from "./generated/Counterfactual";
export type { CounterfactualMode } from "./generated/CounterfactualMode";
export type { Cup } from "./generated/Cup";
export type { CupFormat } from "./generated/CupFormat";
export type { CupPodium } from "./generated/CupPodium";
export type { CupStage } from "./generated/CupStage";
export type { DeletedTournament } from "./generated/DeletedTournament";
export type { EloPriorShape } from "./generated/EloPriorShape";
export type { FloaterStyle } from "./generated/FloaterStyle";
export type { Grade } from "./generated/Grade";
export type { GradeKind } from "./generated/GradeKind";
export type { Handicap } from "./generated/Handicap";
export type { HandicapGame } from "./generated/HandicapGame";
export type { HandicapPolicy } from "./generated/HandicapPolicy";
export type { HealthStatus } from "./generated/HealthStatus";
export type { MacMahonThreshold } from "./generated/MacMahonThreshold";
export type { NewPlayer } from "./generated/NewPlayer";
export type { AbsenceKind } from "./generated/AbsenceKind";
export type { Forfeit } from "./generated/Forfeit";
export type { Outcome } from "./generated/Outcome";
export type { PairingSource } from "./generated/PairingSource";
export type { PairingMode } from "./generated/PairingMode";
export type { EloEstimator } from "./generated/EloEstimator";
export type { MacMahon } from "./generated/MacMahon";
export type { MacMahonSource } from "./generated/MacMahonSource";
export type { Player } from "./generated/Player";
export type { PublicationState } from "./generated/PublicationState";
export type { PublicTournament } from "./generated/PublicTournament";
export type { PublicTournamentResponse } from "./generated/PublicTournamentResponse";
export type { PlayerCategory } from "./generated/PlayerCategory";
export type { PointAdjustment } from "./generated/PointAdjustment";
export type { RatedPlayer } from "./generated/RatedPlayer";
export type { Round } from "./generated/Round";
export type { RoundDraft } from "./generated/RoundDraft";
export type { RoundExplanation } from "./generated/RoundExplanation";
export type { RuleContribution } from "./generated/RuleContribution";
export type { RuleDelta } from "./generated/RuleDelta";
export type { RuleId } from "./generated/RuleId";
export type { RuleTotal } from "./generated/RuleTotal";
export type { ScopeReason } from "./generated/ScopeReason";
export type { Sitout } from "./generated/Sitout";
export type { SitoutKind } from "./generated/SitoutKind";
export type { SitoutValue } from "./generated/SitoutValue";
export type { Standing } from "./generated/Standing";
export type { Team } from "./generated/Team";
export type { TeamSettings } from "./generated/TeamSettings";
export type { TeamStanding } from "./generated/TeamStanding";
export type { ThresholdCriterion } from "./generated/ThresholdCriterion";
export type { Tiebreak } from "./generated/Tiebreak";
export type { Tournament } from "./generated/Tournament";
export type { TournamentResponse } from "./generated/TournamentResponse";
export type { TournamentProblem } from "./generated/TournamentProblem";
export type { TournamentSummary } from "./generated/TournamentSummary";
export type { TournamentSettings } from "./generated/TournamentSettings";
export type { Winner } from "./generated/Winner";

import type { Handicap } from "./generated/Handicap";
import type { Standing } from "./generated/Standing";
import type { Tiebreak } from "./generated/Tiebreak";

/** Handicaps in the picker order, with their display labels. */
export const HANDICAPS: { value: Handicap; label: string }[] = [
  { value: "s", label: "Sente" },
  { value: "l", label: "Lance" },
  { value: "b", label: "Bishop" },
  { value: "r", label: "Rook" },
  { value: "rl", label: "Rook+Lance" },
  { value: "2p", label: "2 pieces" },
  { value: "4p", label: "4 pieces" },
  { value: "5p", label: "5 pieces" },
  { value: "6p", label: "6 pieces" },
];

/** Column label, tooltip, and the `Standing` field for each tie-break metric —
 *  the single source shared by the Settings picker and the Results headers. */
export const TIEBREAKS: {
  code: Tiebreak;
  label: string;
  field: keyof Standing;
  title: string;
}[] = [
  { code: "points", label: "Points", field: "points", title: "Total score: MacMahon start + wins" },
  { code: "sos_m", label: "SOSM", field: "sosm", title: "Sum of opponents' points (MacMahon-inclusive)" },
  { code: "sos_w", label: "SOSW", field: "sosw", title: "Sum of opponents' wins" },
  { code: "sodos_m", label: "SODOSM", field: "sodosm", title: "Sum of defeated opponents' points" },
  { code: "sodos_w", label: "SODOSW", field: "sodosw", title: "Sum of defeated opponents' wins" },
  { code: "sosos_m", label: "SOSOSM", field: "sososm", title: "Sum of opponents' SOSM" },
  { code: "sosos_w", label: "SOSOSW", field: "sososw", title: "Sum of opponents' SOSW" },
  { code: "sos_m1", label: "SOSM-1", field: "sosm1", title: "SOSM dropping the lowest-scoring opponent" },
  { code: "sos_m2", label: "SOSM-2", field: "sosm2", title: "SOSM dropping the two lowest-scoring opponents" },
  { code: "sos_w1", label: "SOSW-1", field: "sosw1", title: "SOSW dropping the lowest-scoring opponent" },
  { code: "sos_w2", label: "SOSW-2", field: "sosw2", title: "SOSW dropping the two lowest-scoring opponents" },
  { code: "cuss_m", label: "CUSSM", field: "cussm", title: "Cumulative sum of the running points total each round" },
  { code: "cuss_w", label: "CUSSW", field: "cussw", title: "Cumulative sum of the running win total each round" },
  { code: "dc", label: "DC", field: "dc", title: "Direct confrontation: wins among players tied on every earlier criterion, when they've all played each other" },
  { code: "est_elo", label: "Est. ELO", field: "estimated_elo", title: "Live Bayesian estimate of the player's current strength" },
  { code: "board_wins", label: "Board wins", field: "victories", title: "Team tournaments: the games a team won across every board, where Points counts the match and not the boards inside it" },
];
