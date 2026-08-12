<!-- The three mutually exclusive formats, and whatever the chosen one adds. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { CupFormat } from "../../types";
  import type { TournamentMode } from "./modes";

  interface Props {
    /** The format in effect, derived by the parent from `teamMode`/`cupEnabled`. */
    tournamentMode: TournamentMode;
    /** Whether teams are the unit of pairing — what finalization freezes. */
    teamMode: boolean;
    teamSize: number;
    cupFormat: CupFormat;
    /** Registration finalized: team mode is frozen from here on. */
    locked: boolean;
    busy: boolean;
    /** Switching format is not a local edit — it retires the pairing mode and
     *  the tie-breaks the other formats have no use for — so it stays with the
     *  parent that owns those. */
    setTournamentMode: (mode: TournamentMode) => void;
    persist: () => void;
  }

  let {
    tournamentMode,
    teamMode,
    teamSize = $bindable(),
    cupFormat = $bindable(),
    locked,
    busy,
    setTournamentMode,
    persist,
  }: Props = $props();

  function setTeamSize(v: number) {
    teamSize = v;
    persist();
  }

  function setCupFormat(v: CupFormat) {
    cupFormat = v;
    persist();
  }
</script>

<section class="section">
  <h3>{$_("settings.tournamentModeTitle")}</h3>
  <!-- Finalization freezes team mode (and only team mode), so an option is
       disabled exactly when picking it would flip it. Switching between the
       normal and cup formats stays open. -->
  <label class="check">
    <input
      type="radio"
      name="tournament-mode"
      value="normal"
      checked={tournamentMode === "normal"}
      disabled={busy || (locked && teamMode)}
      onchange={() => setTournamentMode("normal")}
    />
    {$_("settings.tournamentModeNormal")}
  </label>
  <label class="check">
    <input
      type="radio"
      name="tournament-mode"
      value="team"
      checked={tournamentMode === "team"}
      disabled={busy || (locked && !teamMode)}
      onchange={() => setTournamentMode("team")}
    />
    {$_("settings.tournamentModeTeam")}
  </label>
  <label class="check">
    <input
      type="radio"
      name="tournament-mode"
      value="cup"
      checked={tournamentMode === "cup"}
      disabled={busy || (locked && teamMode)}
      onchange={() => setTournamentMode("cup")}
    />
    {$_("settings.tournamentModeCup")}
  </label>

  {#if tournamentMode === "normal"}
    <p class="desc mode-desc">{$_("settings.tournamentModeNormalDesc")}</p>
  {:else if tournamentMode === "team"}
    <p class="desc mode-desc">{$_("settings.teamDesc")}</p>
    <label class="check">
      {$_("settings.teamSize")}
      <input
        type="number"
        min="2"
        max="9"
        step="1"
        class="threshold narrow"
        value={teamSize}
        disabled={busy || locked}
        onchange={(e) => setTeamSize(Number(e.currentTarget.value))}
      />
    </label>
  {:else}
    <p class="desc mode-desc">{$_("settings.hybridCupDesc")}</p>
    <label class="check">
      <input
        type="radio"
        name="cup-format"
        value="direct"
        checked={cupFormat === "direct"}
        disabled={busy}
        onchange={() => setCupFormat("direct")}
      />
      {$_("settings.cupFormatDirect")}
    </label>
    <label class="check">
      <input
        type="radio"
        name="cup-format"
        value="qualifier"
        checked={cupFormat === "qualifier"}
        disabled={busy}
        onchange={() => setCupFormat("qualifier")}
      />
      {$_("settings.cupFormatQualifier")}
    </label>
    <p class="desc small-note">
      {$_(
        cupFormat === "qualifier"
          ? "settings.cupFormatQualifierDesc"
          : "settings.cupFormatDirectDesc",
      )}
    </p>
  {/if}
  {#if locked}
    <p class="desc small-note">{$_("settings.teamLocked")}</p>
  {/if}
</section>

<style>
  /* The description of the selected tournament mode, between the radios and the
     controls that mode adds. */
  .mode-desc {
    margin: 0.7rem 0 0.5rem;
  }
</style>
