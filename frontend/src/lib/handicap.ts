import type { HandicapPolicy } from "./types";

/**
 * The flat handicap choice the pairings/settings UI works in: no handicaps, the
 * picker, or the picker plus a suggested-handicap column. The wire type
 * ([`HandicapPolicy`]) folds the Wiel rule in alongside it; this is just the
 * display axis, which is all the round view needs.
 */
export type HandicapChoice = "none" | "allowed" | "suggested";

/** Collapse the tagged {@link HandicapPolicy} to its flat display choice. */
export function handicapChoice(policy: HandicapPolicy): HandicapChoice {
  return policy.kind === "enabled" ? policy.display : "none";
}
