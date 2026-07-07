// Display helpers for a board's pairing source (Swiss / Forced / Cup stage).

import type { CupStage, PairingSource } from "./types";

/** A translate function, e.g. the `$_` store from svelte-i18n (its exact
 * parameter type isn't exported, so this is intentionally loose). */
type Translate = (id: string, opts?: any) => string;


/** Human label for a cup stage, e.g. "Round of 32", "Semifinal", "3rd place". */
export function cupStageLabel(stage: CupStage, t: Translate): string {
  if (typeof stage === "object")
    return t("pairingSource.roundOf", { values: { n: stage.round_of } });
  switch (stage) {
    case "quarterfinal":
      return t("pairingSource.quarterfinal");
    case "semifinal":
      return t("pairingSource.semifinal");
    case "final":
      return t("pairingSource.final");
    case "small_final":
      return t("pairingSource.thirdPlace");
  }
}

/** A badge label + kind for a board's pairing source (defaults to Swiss). */
export function sourceBadge(
  t: Translate,
  source?: PairingSource,
): {
  text: string;
  kind: "swiss" | "forced" | "cup";
} {
  if (!source || source.kind === "swiss")
    return { text: t("pairingSource.swiss"), kind: "swiss" };
  if (source.kind === "forced") return { text: t("pairingSource.forced"), kind: "forced" };
  return { text: cupStageLabel(source.stage, t), kind: "cup" };
}
