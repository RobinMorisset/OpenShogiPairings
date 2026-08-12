<!-- Swiss or the experimental pure-ELO pairing. Only offered for the normal
     format; the other two are Swiss by construction. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import EstimatorKnobs from "./EstimatorKnobs.svelte";

  interface Props {
    /** Which of the two modes is in effect. */
    pairingMode: "swiss" | "elo";
    /** Same thing, as the flag the estimator knobs hang off. */
    eloEnabled: boolean;
    eloApplyTo: "unrated" | "all";
    unratedPrior: "flat" | "laplace";
    busy: boolean;
    /** Leaving or entering the ELO mode also seeds the estimator and retires the
     *  estimate tie-break, so the parent owns it. */
    setPairingMode: (mode: "swiss" | "elo") => void;
    setEloApplyTo: (v: "unrated" | "all") => void;
    setUnratedPrior: (v: "flat" | "laplace") => void;
  }

  let {
    pairingMode,
    eloEnabled,
    eloApplyTo,
    unratedPrior,
    busy,
    setPairingMode,
    setEloApplyTo,
    setUnratedPrior,
  }: Props = $props();
</script>

<section class="section">
  <h3>{$_("settings.pairingModeTitle")}</h3>
  <p class="desc">
    {$_("settings.pairingModeDesc")}
  </p>
  <label class="check">
    <input
      type="radio"
      name="pairing-mode"
      value="swiss"
      checked={pairingMode === "swiss"}
      disabled={busy}
      onchange={() => setPairingMode("swiss")}
    />
    {$_("settings.pairingModeSwiss")}
  </label>
  <label class="check">
    <input
      type="radio"
      name="pairing-mode"
      value="elo"
      checked={pairingMode === "elo"}
      disabled={busy}
      onchange={() => setPairingMode("elo")}
    />
    {$_("settings.pairingModeElo")}
  </label>
  <p class="desc small-note">
    {$_("settings.eloModeDesc")}
  </p>
  {#if eloEnabled}
    <EstimatorKnobs
      {eloApplyTo}
      {unratedPrior}
      {busy}
      {setEloApplyTo}
      {setUnratedPrior}
    />
  {/if}
</section>
