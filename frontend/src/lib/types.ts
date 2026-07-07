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
  fesa_games?: number; // FESA #games behind the rating (reliability), if from the list
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
  fesa_games?: number; // FESA #games, set when the rating was picked from the list
  nationality?: string;
  club?: string;
}

/** An entry from the FESA rating list — mirror of `osp_core::RatedPlayer`. */
export interface RatedPlayer {
  last_name: string;
  first_name: string;
  rating: number;
  games: number; // number of rated games behind the rating
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
  /**
   * Tie-breaks used to rank the standings, in priority order (points is always
   * the primary key). Only these columns show on the Results tab. Defaults to
   * the classic SOSM → SODOSM → SOSOSM order.
   */
  tiebreaks: Tiebreak[];
  /**
   * Experimental ELO-based (non-Swiss) pairing mode. When on, MacMahon, the
   * Swiss-specific rules and club protection are disabled; pairing minimizes the
   * squared difference of a live Bayesian ELO estimate. Off by default.
   */
  elo_pairing_enabled: boolean;
  /**
   * The ELO-estimate K multiplier as an integer percent (100 = ×1.0): how far
   * the estimate may drift from the registration rating. Only meaningful when
   * `elo_pairing_enabled`; expected range ~100–400.
   */
  elo_k_multiplier_percent: number;
  /**
   * Extra K multiplier (integer percent, 200 = ×2.0) for a provisionally-rated
   * player — not in the FESA list, or fewer than 18 FESA games — widening their
   * prior so their estimate drifts faster. Only meaningful when
   * `elo_pairing_enabled`.
   */
  elo_provisional_multiplier_percent: number;
}

/** Mirror of `osp_core::HandicapPolicy`. */
export type HandicapPolicy = "none" | "allowed" | "suggested";

/** Mirror of `osp_core::Tiebreak` (serde snake_case codes). Points is a ranking
 *  criterion like the rest; each opponent-sum metric has a MacMahon-inclusive
 *  (`…_m`) and a wins-only (`…_w`) flavour. */
export type Tiebreak =
  | "points"
  | "sos_m"
  | "sos_w"
  | "sodos_m"
  | "sodos_w"
  | "sosos_m"
  | "sosos_w"
  | "sos_m1"
  | "sos_m2"
  | "sos_w1"
  | "sos_w2"
  | "cuss_m"
  | "cuss_w"
  | "est_elo";

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
  { code: "est_elo", label: "Est. ELO", field: "estimated_elo", title: "Live Bayesian estimate of the player's current strength" },
];

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

/** One player's standing — mirror of `osp_core::Standing`. All twelve tie-break
 *  metrics are always present; the settings decide which are shown/used. */
export interface Standing {
  player_id: string; // UUID
  victories: number;
  macmahon: number;
  points: number; // victories + macmahon
  sosm: number; // sum of opponents' points
  sosw: number; // sum of opponents' wins
  sodosm: number; // sum of defeated opponents' points
  sodosw: number; // sum of defeated opponents' wins
  sososm: number; // sum of opponents' SOSM
  sososw: number; // sum of opponents' SOSW
  sosm1: number; // SOSM dropping the lowest opponent
  sosm2: number; // SOSM dropping the two lowest opponents
  sosw1: number; // SOSW dropping the lowest opponent
  sosw2: number; // SOSW dropping the two lowest opponents
  cussm: number; // cumulative running points total
  cussw: number; // cumulative running win total
  estimated_elo: number; // live Bayesian ELO estimate (shown in ELO mode)
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
