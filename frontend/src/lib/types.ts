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

/** Mirror of `osp_core::Board` — one game in a round. */
export interface Board {
  player1: string; // player UUID
  player2: string; // player UUID
}

/** Mirror of `osp_core::Round`. */
export interface Round {
  number: number;
  boards: Board[];
  bye?: string; // player UUID sitting out
}

/** Mirror of `osp_core::Tournament`. */
export interface Tournament {
  format_version: number;
  id: string; // UUID
  name: string;
  players: Player[];
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
