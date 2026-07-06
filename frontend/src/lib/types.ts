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

/** Mirror of `osp_core::Board` — one game in a round. */
export interface Board {
  player1: string; // player UUID
  player2: string; // player UUID
  result?: Winner; // absent = not played yet
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

/** Mirror of `osp_core::Tournament`. */
export interface Tournament {
  format_version: number;
  id: string; // UUID
  name: string;
  players: Player[];
  registration_finalized: boolean;
  draft?: RoundDraft | null;
  rounds: Round[];
}

/**
 * API response for tournament endpoints: the tournament plus whether an undo is
 * available. Kept separate from `Tournament` so the saved-file shape stays clean.
 */
export interface TournamentResponse {
  tournament: Tournament;
  can_undo: boolean;
}
