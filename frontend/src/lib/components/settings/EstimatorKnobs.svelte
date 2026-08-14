<!-- The two controls of the live ELO estimate. Rendered next to whichever switch
     turns the estimate on — the pure ELO pairing mode, or estimate-based
     MacMahon — rather than in one fixed place, so the knobs are never in a
     different section from the option that gives them an effect. -->
<script lang="ts">
  import { _ } from "svelte-i18n";

  interface Props {
    /** Who the estimate applies to: the unrated only, or everyone. */
    eloApplyTo: "unrated" | "all";
    /** The unrated prior: the flat performance rating, or the tuned Laplace. */
    unratedPrior: "flat" | "laplace";
    busy: boolean;
    setEloApplyTo: (v: "unrated" | "all") => void;
    setUnratedPrior: (v: "flat" | "laplace") => void;
  }

  let { eloApplyTo, unratedPrior, busy, setEloApplyTo, setUnratedPrior }: Props = $props();
</script>

<div class="estimator">
  <p class="desc small-note">
    {$_("settings.eloEstimateKnobsNote")}
  </p>
  <label class="check elo-k">
    {$_("settings.eloApplyTo")}
    <select
      class="tb-select control-sm control-quiet"
      value={eloApplyTo}
      disabled={busy}
      onchange={(e) => setEloApplyTo(e.currentTarget.value as "unrated" | "all")}
    >
      <option value="unrated">{$_("settings.eloApplyToUnrated")}</option>
      <option value="all">{$_("settings.eloApplyToAll")}</option>
    </select>
  </label>
  <p class="desc small-note">
    {$_("settings.eloApplyToDesc")}
  </p>
  <label class="check elo-k">
    {$_("settings.eloUnratedPrior")}
    <select
      class="tb-select control-sm control-quiet"
      value={unratedPrior}
      disabled={busy}
      onchange={(e) => setUnratedPrior(e.currentTarget.value as "flat" | "laplace")}
    >
      <option value="flat">{$_("settings.eloUnratedPriorFlat")}</option>
      <option value="laplace">{$_("settings.eloUnratedPriorLaplace")}</option>
    </select>
  </label>
  <p class="desc small-note">
    {$_("settings.eloUnratedPriorDesc")}
  </p>
</div>

<style>
  .estimator {
    margin-top: 0.7rem;
  }
  .elo-k {
    margin-top: 0.7rem;
  }
</style>
