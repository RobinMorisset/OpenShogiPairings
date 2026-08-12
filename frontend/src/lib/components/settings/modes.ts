/** The three mutually exclusive tournament formats, as one radio group.
 *
 * Team mode is exclusive by the server's own rule (`team_mode_conflict` rejects
 * the cup and the ELO pairing alongside it); the hybrid cup is exclusive here by
 * choice — a bracket exists to find a single winner, which is the opposite of
 * what the pure ELO mode optimizes for, so the cup implies the Swiss pairing.
 */
export type TournamentMode = "normal" | "team" | "cup";
