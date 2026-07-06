// Display helpers for a board's pairing source (Swiss / Forced / Cup stage).

import type { CupStage, PairingSource } from "./types";

/** Human label for a cup stage, e.g. "Round of 32", "Semifinal", "3rd place". */
export function cupStageLabel(stage: CupStage): string {
  if (typeof stage === "object") return `Round of ${stage.round_of}`;
  switch (stage) {
    case "quarterfinal":
      return "Quarterfinal";
    case "semifinal":
      return "Semifinal";
    case "final":
      return "Final";
    case "small_final":
      return "3rd place";
  }
}

/** A badge label + kind for a board's pairing source (defaults to Swiss). */
export function sourceBadge(source?: PairingSource): {
  text: string;
  kind: "swiss" | "forced" | "cup";
} {
  if (!source || source.kind === "swiss") return { text: "Swiss", kind: "swiss" };
  if (source.kind === "forced") return { text: "Forced", kind: "forced" };
  return { text: cupStageLabel(source.stage), kind: "cup" };
}
