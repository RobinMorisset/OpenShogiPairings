<!-- The Swiss rules that are neither MacMahon nor ranking: who floats, and the
     two affiliation protections. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { Player } from "../../types";

  interface Props {
    floaterStyle: "classic" | "median";
    clubEnabled: boolean;
    /** Rounds the protection applies to, `null` for "every round". */
    clubRounds: number | null;
    exemptClubs: string[];
    /** Nationality protection: the same three controls, one rule tier weaker. */
    nationalityEnabled: boolean;
    nationalityRounds: number | null;
    exemptNationalities: string[];
    /** The registered players, used to suggest club and nationality names. */
    players: Player[];
    busy: boolean;
    persist: () => void;
  }

  let {
    floaterStyle = $bindable(),
    clubEnabled = $bindable(),
    clubRounds = $bindable(),
    exemptClubs = $bindable(),
    nationalityEnabled = $bindable(),
    nationalityRounds = $bindable(),
    exemptNationalities = $bindable(),
    players,
    busy,
    persist,
  }: Props = $props();

  // Distinct values of one player field (first spelling kept) with their player
  // count, for an exempt datalist — sorted by decreasing count (ties broken
  // alphabetically) so the entries most worth exempting sort first.
  function distinctValues(pick: (p: Player) => string | null | undefined) {
    const counts = new Map<string, { name: string; count: number }>();
    for (const p of players) {
      const name = pick(p)?.trim();
      if (!name) continue;
      const key = name.toLowerCase();
      const existing = counts.get(key);
      if (existing) existing.count++;
      else counts.set(key, { name, count: 1 });
    }
    return [...counts.values()].sort(
      (a, b) => b.count - a.count || a.name.localeCompare(b.name),
    );
  }

  const knownClubs = $derived.by(() => distinctValues((p) => p.club));
  const knownNationalities = $derived.by(() => distinctValues((p) => p.nationality));

  function setFloaterStyle(v: "classic" | "median") {
    floaterStyle = v;
    persist();
  }

  function setClubEnabled(v: boolean) {
    clubEnabled = v;
    persist();
  }

  function setRoundLimit(limited: boolean) {
    clubRounds = limited ? (clubRounds ?? 1) : null;
    persist();
  }

  /** A round-window input's value, floored at the first round. */
  function roundLimit(raw: string): number {
    const n = Math.round(Number(raw));
    return Number.isFinite(n) && n >= 1 ? n : 1;
  }

  function editClubRounds(raw: string) {
    clubRounds = roundLimit(raw);
    persist();
  }

  function addExempt() {
    exemptClubs.push("");
    persist();
  }

  function removeExempt(i: number) {
    exemptClubs.splice(i, 1);
    persist();
  }

  function editExempt(i: number, raw: string) {
    exemptClubs[i] = raw;
    persist();
  }

  function setNationalityEnabled(v: boolean) {
    nationalityEnabled = v;
    persist();
  }

  function setNationalityRoundLimit(limited: boolean) {
    nationalityRounds = limited ? (nationalityRounds ?? 1) : null;
    persist();
  }

  function editNationalityRounds(raw: string) {
    nationalityRounds = roundLimit(raw);
    persist();
  }

  function addExemptNationality() {
    exemptNationalities.push("");
    persist();
  }

  function removeExemptNationality(i: number) {
    exemptNationalities.splice(i, 1);
    persist();
  }

  function editExemptNationality(i: number, raw: string) {
    exemptNationalities[i] = raw;
    persist();
  }
</script>

<section class="section">
  <h3>{$_("settings.otherPairingRulesTitle")}</h3>
  <div class="grid rules-grid">
    <fieldset class="sub">
      <legend>{$_("settings.floaterTitle")}</legend>
      <p class="desc">
        {$_("settings.floaterDesc")}
      </p>
      <label class="check">
        <input
          type="radio"
          name="floater-style"
          value="classic"
          checked={floaterStyle === "classic"}
          disabled={busy}
          onchange={() => setFloaterStyle("classic")}
        />
        {$_("settings.floaterClassic")}
      </label>
      <label class="check">
        <input
          type="radio"
          name="floater-style"
          value="median"
          checked={floaterStyle === "median"}
          disabled={busy}
          onchange={() => setFloaterStyle("median")}
        />
        {$_("settings.floaterMedian")}
      </label>
    </fieldset>

    <fieldset class="sub">
      <legend>{$_("settings.clubProtectionTitle")}</legend>
      <p class="desc">
        {$_("settings.clubProtectionDesc")}
      </p>
      <label class="check">
        <input
          type="checkbox"
          checked={clubEnabled}
          disabled={busy}
          onchange={(e) => setClubEnabled(e.currentTarget.checked)}
        />
        {$_("settings.clubProtectionCheckbox")}
      </label>

      {#if clubEnabled}
        <div class="club-sub">
          <label class="check">
            <input
              type="checkbox"
              checked={clubRounds != null}
              disabled={busy}
              onchange={(e) => setRoundLimit(e.currentTarget.checked)}
            />
            {$_("settings.onlyFirstRoundsPrefix")}
            <input
              type="number"
              min="1"
              step="1"
              class="threshold narrow control-sm control-quiet"
              value={clubRounds ?? 1}
              disabled={busy || clubRounds == null}
              onchange={(e) => editClubRounds(e.currentTarget.value)}
            />
            {$_("settings.onlyFirstRoundsSuffix")}
          </label>

          <p class="desc exempt-desc">
            {$_("settings.exemptDesc")}
          </p>
          <div class="thresholds">
            {#each exemptClubs as c, i (i)}
              <div class="threshold-row">
                <input
                  type="text"
                  class="club-input control-sm control-quiet"
                  list="known-clubs"
                  placeholder={$_("settings.clubNamePlaceholder")}
                  value={c}
                  disabled={busy}
                  onchange={(e) => editExempt(i, e.currentTarget.value)}
                />
                <button
                  type="button"
                  class="remove"
                  disabled={busy}
                  title={$_("settings.removeExemption")}
                  onclick={() => removeExempt(i)}>✕</button
                >
              </div>
            {/each}
            {#if exemptClubs.length === 0}
              <p class="muted">{$_("settings.noExemptions")}</p>
            {/if}
            <button
              type="button"
              class="ghost control-xs control-quiet"
              disabled={busy}
              onclick={addExempt}>{$_("settings.addExemptClub")}</button
            >
          </div>
          {#if knownClubs.length > 0}
            <datalist id="known-clubs">
              {#each knownClubs as club (club.name)}
                <!-- Label is the count *alone*: browsers already show the value
                     (it is what selecting inserts) and add the label beside it
                     only when the two differ, so repeating the name here shows it
                     twice. Text content would be ignored outright — a `label`
                     attribute supersedes it. -->
                <option value={club.name} label={`(${club.count})`}></option>
              {/each}
            </datalist>
          {/if}
        </div>
      {/if}
    </fieldset>

    <fieldset class="sub">
      <legend>{$_("settings.nationalityProtectionTitle")}</legend>
      <p class="desc">
        {$_("settings.nationalityProtectionDesc")}
      </p>
      <label class="check">
        <input
          type="checkbox"
          checked={nationalityEnabled}
          disabled={busy}
          onchange={(e) => setNationalityEnabled(e.currentTarget.checked)}
        />
        {$_("settings.nationalityProtectionCheckbox")}
      </label>

      {#if nationalityEnabled}
        <div class="club-sub">
          <label class="check">
            <input
              type="checkbox"
              checked={nationalityRounds != null}
              disabled={busy}
              onchange={(e) => setNationalityRoundLimit(e.currentTarget.checked)}
            />
            {$_("settings.onlyFirstRoundsPrefix")}
            <input
              type="number"
              min="1"
              step="1"
              class="threshold narrow control-sm control-quiet"
              value={nationalityRounds ?? 1}
              disabled={busy || nationalityRounds == null}
              onchange={(e) => editNationalityRounds(e.currentTarget.value)}
            />
            {$_("settings.onlyFirstRoundsSuffix")}
          </label>

          <p class="desc exempt-desc">
            {$_("settings.exemptNationalityDesc")}
          </p>
          <div class="thresholds">
            {#each exemptNationalities as c, i (i)}
              <div class="threshold-row">
                <input
                  type="text"
                  class="club-input control-sm control-quiet"
                  list="known-nationalities"
                  placeholder={$_("settings.nationalityPlaceholder")}
                  value={c}
                  disabled={busy}
                  onchange={(e) => editExemptNationality(i, e.currentTarget.value)}
                />
                <button
                  type="button"
                  class="remove"
                  disabled={busy}
                  title={$_("settings.removeExemption")}
                  onclick={() => removeExemptNationality(i)}>✕</button
                >
              </div>
            {/each}
            {#if exemptNationalities.length === 0}
              <p class="muted">{$_("settings.noNationalityExemptions")}</p>
            {/if}
            <button
              type="button"
              class="ghost control-xs control-quiet"
              disabled={busy}
              onclick={addExemptNationality}>{$_("settings.addExemptNationality")}</button
            >
          </div>
          {#if knownNationalities.length > 0}
            <datalist id="known-nationalities">
              {#each knownNationalities as nat (nat.name)}
                <option value={nat.name} label={`(${nat.count})`}></option>
              {/each}
            </datalist>
          {/if}
        </div>
      {/if}
    </fieldset>
  </div>
</section>

<style>
  .rules-grid {
    --col-min: 20rem;
  }
  .club-sub {
    margin: 0.8rem 0 0 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .exempt-desc {
    margin: 0;
  }
  .club-input {
    width: 12rem;
    max-width: 100%;
  }
</style>
