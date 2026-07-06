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
  name: string;
  rating?: number;
  club?: string;
}

/** Registration payload — mirror of `osp_core::NewPlayer`. */
export interface NewPlayer {
  name: string;
  rating?: number;
  club?: string;
}

/** Mirror of `osp_core::Tournament`. */
export interface Tournament {
  format_version: number;
  id: string; // UUID
  name: string;
  players: Player[];
}
