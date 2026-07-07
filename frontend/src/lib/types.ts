// Client-side mirrors of the types the server returns.
//
// For now these are maintained by hand. Once the API surface grows, we should
// generate them from the Rust `osp-core` types (e.g. with `ts-rs`) so the
// contract can never silently drift between server and clients.

/** Mirror of `osp_core::HealthStatus`. */
export interface HealthStatus {
  status: string;
  service: string;
  version: string;
}

/** Mirror of `osp_core::Player`. */
export interface Player {
  id: string; // UUID
  tournament_id?: number; // human-facing number, assigned at finalize
  last_name: string;
  first_name: string;
  rating?: number;
  nationality?: string; // country code, e.g. "JP"
  club?: string;
  eligible?: boolean; // eligible for the direct-elimination cup
  adjustments?: PointAdjustment[]; // manual point bonuses/maluses
}

/** Mirror of `osp_core::PointAdjustment`. */
export interface PointAdjustment {
  id: string; // UUID
  delta: number; // positive = bonus, negative = malus
  reason: string;
}

/** Registration payload — mirror of `osp_core::NewPlayer`. */
export interface NewPlayer {
  last_name: string;
  first_name?: string;
  rating?: number;
  nationality?: string;
  club?: string;
}

/** An entry from the FESA rating list — mirror of `osp_core::RatedPlayer`. */
export interface RatedPlayer {
  last_name: string;
  first_name: string;
  rating: number;
  nationality: string;
}

/** Which player won a board — mirror of `osp_core::Winner`. */
export type Winner = "player1" | "player2";

/** A piece-odds handicap — mirror of `osp_core::Handicap` (serialized as its code). */
export type Handicap =
  | "s"
  | "l"
  | "b"
  | "r"
  | "rl"
  | "2p"
  | "4p"
  | "5p"
  | "6p";

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

/** A handicap attached to a board — mirror of `osp_core::HandicapGame`. */
export interface HandicapGame {
  handicap: Handicap;
  giver: Winner; // frozen: the higher-rated player
}

/** Which stage of the cup a board belongs to — mirror of `osp_core::CupStage`. */
export type CupStage =
  | { round_of: number }
  | "quarterfinal"
  | "semifinal"
  | "final"
  | "small_final";

/** How a board was paired — mirror of `osp_core::PairingSource` (internally tagged). */
export type PairingSource =
  | { kind: "swiss" }
  | { kind: "forced" }
  | { kind: "cup"; stage: CupStage };

/** Mirror of `osp_core::Board` — one game in a round. */
export interface Board {
  player1: string; // player UUID
  player2: string; // player UUID
  result?: Winner; // actual winner; absent = not played yet
  drawn?: boolean; // a draw occurred before the decisive game
  handicap?: HandicapGame | null; // piece odds, if any
  points_diff?: number | null; // points(p1) − points(p2) frozen at pairing time
  source?: PairingSource; // how the pairing was decided; absent = swiss
}

/** Mirror of `osp_core::Round`. */
export interface Round {
  number: number;
  boards: Board[];
  bye?: string; // player UUID sitting out
  absent: string[]; // player UUIDs marked absent
  completed: boolean;
}

/** Mirror of `osp_core::RoundDraft` — a round being set up but not yet started. */
export interface RoundDraft {
  number: number;
  absent: string[]; // player UUIDs
  forced_boards: Board[]; // { player1, player2 }
  forced_bye?: string; // player UUID
}

/** Mirror of `osp_core::TournamentSettings`. */
export interface TournamentSettings {
  /** ELO thresholds (ascending) defining the MacMahon starting groups. */
  macmahon_thresholds: number[];
  /**
   * Degressive MacMahon: round numbers at whose end one bottom threshold is
   * dropped. Sorted ascending; a repeated round drops several at once. Length is
   * capped server-side at the threshold count.
   */
  macmahon_removals: number[];
  /** Whether pairings avoid same-club players (off by default). */
  club_protection_enabled: boolean;
  /** If set, club protection applies only to rounds 1..=n; null/absent = all. */
  club_protection_rounds?: number | null;
  /** Clubs exempt from protection (the "local club"); matched case-insensitively. */
  club_protection_exempt_clubs: string[];
  /** Which player each group floats up: "classic" (first) or "median" Swiss. */
  floater_style: "classic" | "median";
  /** Whether this is a hybrid tournament with a direct-elimination cup. */
  cup_enabled: boolean;
  /** How handicap games are treated: hidden, allowed, or suggested. */
  handicap_policy: HandicapPolicy;
}

/** Mirror of `osp_core::HandicapPolicy`. */
export type HandicapPolicy = "none" | "allowed" | "suggested";

/** Mirror of `osp_core::Cup` — the seeded direct-elimination bracket. */
export interface Cup {
  size: number; // 8/16/32/64
  seed_order: string[]; // player UUIDs, seed 1..size
}

/** Mirror of `osp_core::CupPodium` — decided once the final round is played. */
export interface CupPodium {
  champion: string; // player UUID
  runner_up: string;
  third: string;
  fourth: string;
}

/** Mirror of `osp_core::Tournament`. */
export interface Tournament {
  format_version: number;
  id: string; // UUID
  name: string;
  settings: TournamentSettings;
  players: Player[];
  registration_finalized: boolean;
  draft?: RoundDraft | null;
  rounds: Round[];
  cup?: Cup | null;
}

/** One player's standing — mirror of `osp_core::Standing`. */
export interface Standing {
  player_id: string; // UUID
  victories: number;
  macmahon: number;
  points: number; // victories + macmahon
  sos: number; // sum of opponents' points
  sodos: number; // sum of defeated opponents' points
  sosos: number; // sum of opponents' SOS
}

/** One automatic server-side backup's metadata — mirror of `osp_server`'s
 *  `BackupInfo`. Taken at round state-machine transitions (finalize, prepare,
 *  confirm, complete, cancel); the tournament body is only fetched on restore. */
export interface BackupInfo {
  id: string; // opaque, used to restore this backup
  taken_at: number; // Unix seconds
  label: string; // e.g. "round 2 started"
}

/**
 * API response for tournament endpoints: the tournament, whether an undo is
 * available, and the server-computed ranked standings. Kept separate from
 * `Tournament` so the saved-file shape stays clean.
 */
export interface TournamentResponse {
  tournament: Tournament;
  can_undo: boolean;
  standings: Standing[];
  cup_podium?: CupPodium | null; // present once the cup final is decided
  draft_cup_players?: string[]; // players the cup pairs in the round being drafted
  /**
   * Suggested handicap per board, indexed like `tournament.rounds[i].boards[j]`.
   * Computed regardless of `handicap_policy`; `null` = no suggestion (near-equal
   * strength, an unrated player, or a cup board).
   */
  suggested_handicaps: (Handicap | null)[][];
}
