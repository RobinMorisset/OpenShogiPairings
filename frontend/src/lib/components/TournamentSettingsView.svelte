<script lang="ts">
  import { untrack } from "svelte";
  import type { HandicapPolicy, Player, TournamentSettings } from "../types";

  interface Props {
    settings: TournamentSettings;
    /** Registration already finalized — edits here get a warning. */
    finalized: boolean;
    /** The registered players, used to suggest club names for exemptions. */
    players: Player[];
    onUpdate: (settings: TournamentSettings) => void;
    busy?: boolean;
  }

  let { settings, finalized, players, onUpdate, busy = false }: Props = $props();

  // Distinct club names among the players (first spelling kept), for the exempt
  // datalist.
  const knownClubs = $derived.by(() => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const p of players) {
      const c = p.club?.trim();
      if (c && !seen.has(c.toLowerCase())) {
        seen.add(c.toLowerCase());
        out.push(c);
      }
    }
    return out.sort((a, b) => a.localeCompare(b));
  });

  // Keep only positive integers, then sort ascending — the server's canonical
  // order, used to compare our local state against what it stored.
  function cleanSorted(arr: number[]): number[] {
    return arr
      .filter((v) => Number.isFinite(v) && v >= 1)
      .map((v) => Math.round(v))
      .sort((a, b) => a - b);
  }

  function eq(a: number[], b: number[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
  }

  function eqStr(a: string[], b: string[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
  }

  // Mirror the server's exempt-club normalization: trim, drop empties, and
  // de-duplicate case-insensitively keeping the first spelling.
  function normExempt(list: string[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const raw of list) {
      const c = raw.trim();
      if (c && !seen.has(c.toLowerCase())) {
        seen.add(c.toLowerCase());
        out.push(c);
      }
    }
    return out;
  }

  // Local editable rows, kept in *entry* order (not sorted) so the row a referee
  // is editing never jumps or shows a stale value. The inputs bind to these.
  let thresholds = $state<number[]>([]);
  let removals = $state<number[]>([]);
  let clubEnabled = $state(false);
  let clubRounds = $state<number | null>(null);
  let exemptClubs = $state<string[]>([]);
  let floaterStyle = $state<"classic" | "median">("classic");
  let cupEnabled = $state(false);
  let handicapPolicy = $state<HandicapPolicy>("allowed");

  // Adopt the persisted settings only on a genuine external change — a load, an
  // undo, or the server normalizing our input. When our own edit merely
  // round-trips (the server just sorts/dedups it into the same canonical form),
  // our local state already matches, so we keep the entry order rather than
  // reshuffling under the cursor. `untrack` makes this fire on `settings` changes
  // only, not our own writes.
  $effect(() => {
    const sThresholds = settings.macmahon_thresholds;
    const sRemovals = settings.macmahon_removals;
    const sEnabled = settings.club_protection_enabled;
    const sRounds = settings.club_protection_rounds ?? null;
    const sExempt = settings.club_protection_exempt_clubs;
    const sFloater = settings.floater_style;
    const sCup = settings.cup_enabled;
    const sHandicap = settings.handicap_policy;
    untrack(() => {
      const matches =
        eq(cleanSorted(thresholds), sThresholds) &&
        eq(cleanSorted(removals), sRemovals) &&
        clubEnabled === sEnabled &&
        (clubRounds ?? null) === sRounds &&
        eqStr(normExempt(exemptClubs), sExempt) &&
        floaterStyle === sFloater &&
        cupEnabled === sCup &&
        handicapPolicy === sHandicap;
      if (!matches) {
        thresholds = [...sThresholds];
        removals = [...sRemovals];
        clubEnabled = sEnabled;
        clubRounds = sRounds;
        exemptClubs = [...sExempt];
        floaterStyle = sFloater;
        cupEnabled = sCup;
        handicapPolicy = sHandicap;
      }
    });
  });

  function persist() {
    // Send the current values; the server normalizes them (sorts/de-dups the
    // MacMahon lists, caps the removals, trims/de-dups the exempt clubs).
    onUpdate({
      macmahon_thresholds: thresholds
        .filter((v) => Number.isFinite(v) && v >= 1)
        .map((v) => Math.round(v)),
      macmahon_removals: removals
        .filter((v) => Number.isFinite(v) && v >= 1)
        .map((v) => Math.round(v)),
      club_protection_enabled: clubEnabled,
      club_protection_rounds: clubRounds,
      club_protection_exempt_clubs: exemptClubs
        .map((c) => c.trim())
        .filter((c) => c.length > 0),
      floater_style: floaterStyle,
      cup_enabled: cupEnabled,
      handicap_policy: handicapPolicy,
    });
  }

  function setFloaterStyle(v: "classic" | "median") {
    floaterStyle = v;
    persist();
  }

  function setCupEnabled(v: boolean) {
    cupEnabled = v;
    persist();
  }

  function setHandicapPolicy(v: HandicapPolicy) {
    handicapPolicy = v;
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

  function editClubRounds(raw: string) {
    const n = Math.round(Number(raw));
    clubRounds = Number.isFinite(n) && n >= 1 ? n : 1;
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

  function addThreshold() {
    thresholds.push(thresholds.length ? Math.max(...thresholds) + 100 : 1500);
    persist();
  }

  function removeThreshold(i: number) {
    thresholds.splice(i, 1);
    persist();
  }

  function editThreshold(i: number, raw: string) {
    thresholds[i] = Number(raw);
    persist();
  }

  function addRemoval() {
    // Default to the round of the last removal (so repeated clicks stack drops on
    // the same round), or round 1 for the first one.
    removals.push(removals.length ? Math.max(...removals) : 1);
    persist();
  }

  function removeRemoval(i: number) {
    removals.splice(i, 1);
    persist();
  }

  function editRemoval(i: number, raw: string) {
    removals[i] = Number(raw);
    persist();
  }

  // Normalized (sorted, de-duplicated, capped) view of the local rows — drives the
  // previews so they always reflect what the server will store, updated live on
  // each edit rather than lagging a round-trip.
  const normThresholds = $derived.by(() => {
    const sorted = cleanSorted(thresholds);
    return sorted.filter((v, i) => i === 0 || v !== sorted[i - 1]);
  });
  const normRemovals = $derived(cleanSorted(removals).slice(0, normThresholds.length));

  // Can't schedule more removals than there are (distinct) thresholds to drop.
  const canAddRemoval = $derived(removals.length < normThresholds.length);

  // Preview of the resulting bands, e.g. "below 1200 → 0".
  const bands = $derived.by(() => {
    const t = normThresholds;
    const rows: { label: string; points: number }[] = [];
    if (t.length === 0) return rows;
    rows.push({ label: `below ${t[0]}`, points: 0 });
    for (let i = 0; i < t.length; i++) {
      const hi = t[i + 1];
      rows.push({
        label: hi != null ? `${t[i]}–${hi - 1}` : `${t[i]} and above`,
        points: i + 1,
      });
    }
    return rows;
  });

  // Preview of how the starting-point spread shrinks over the rounds, driven by
  // the normalized removal schedule.
  const schedule = $derived.by(() => {
    const total = normThresholds.length;
    const rem = normRemovals;
    if (total === 0 || rem.length === 0) return [];
    const counts = new Map<number, number>();
    for (const r of rem) counts.set(r, (counts.get(r) ?? 0) + 1);
    const rounds = [...counts.keys()].sort((a, b) => a - b);
    const rows: { label: string; max: number }[] = [];
    let active = total;
    let start = 1;
    for (const r of rounds) {
      rows.push({ label: start === r ? `round ${r}` : `rounds ${start}–${r}`, max: active });
      active = Math.max(0, active - (counts.get(r) ?? 0));
      start = r + 1;
    }
    rows.push({ label: `round ${start}+`, max: active });
    return rows;
  });
</script>

<div class="settings">
  {#if finalized}
    <p class="hint warning">
      ⚠ Registration is finalized — changing the MacMahon groups will change
      everyone's points and future pairings.
    </p>
  {/if}

  <h3>MacMahon groups</h3>
  <p class="desc">
    Each player starts with one point per ELO threshold their rating reaches. For
    example, with thresholds 1200 and 1700 a player rated below 1200 starts at 0,
    from 1200 to 1699 at 1, and 1700 or above at 2. Unrated players start at 0.
    Leave the list empty to disable MacMahon.
  </p>

  <div class="thresholds">
    {#each thresholds as v, i (i)}
      <div class="threshold-row">
        <input
          type="number"
          min="1"
          step="1"
          class="threshold"
          value={v}
          disabled={busy}
          onchange={(e) => editThreshold(i, e.currentTarget.value)}
        />
        <button
          type="button"
          class="remove"
          disabled={busy}
          title="Remove this threshold"
          onclick={() => removeThreshold(i)}>✕</button
        >
      </div>
    {/each}
    {#if thresholds.length === 0}
      <p class="muted">No thresholds — every player starts at 0 MacMahon points.</p>
    {/if}
    <button
      type="button"
      class="ghost small"
      disabled={busy}
      onclick={addThreshold}>Add threshold</button
    >
  </div>

  {#if bands.length > 0}
    <div class="preview">
      <h4>Starting points</h4>
      <ul>
        {#each bands as b (b.points)}
          <li><span class="band">{b.label}</span> → <strong>{b.points}</strong></li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if thresholds.length > 0}
    <div class="section">
      <h3>Degressive MacMahon</h3>
      <p class="desc">
        Also called <em>accelerated Swiss</em>: drop the lowest MacMahon group at
        the end of a round, so the starting-point head start fades as the field
        converges. Schedule one entry per group to drop; two entries on the same
        round drop two groups at once. A drop scheduled after round N takes effect
        from round N+1.
      </p>

      <div class="thresholds">
        {#each removals as r, i (i)}
          <div class="threshold-row">
            <span class="prefix">Drop one group after round</span>
            <input
              type="number"
              min="1"
              step="1"
              class="threshold narrow"
              value={r}
              disabled={busy}
              onchange={(e) => editRemoval(i, e.currentTarget.value)}
            />
            <button
              type="button"
              class="remove"
              disabled={busy}
              title="Remove this drop"
              onclick={() => removeRemoval(i)}>✕</button
            >
          </div>
        {/each}
        {#if removals.length === 0}
          <p class="muted">No drops — MacMahon groups stay fixed all tournament.</p>
        {/if}
        <button
          type="button"
          class="ghost small"
          disabled={busy || !canAddRemoval}
          title={canAddRemoval ? "" : "Can't drop more groups than there are thresholds"}
          onclick={addRemoval}>Add drop</button
        >
      </div>

      {#if schedule.length > 0}
        <div class="preview">
          <h4>Spread over rounds</h4>
          <ul>
            {#each schedule as s (s.label)}
              <li>
                <span class="band">{s.label}</span> → up to
                <strong>{s.max}</strong>
                starting {s.max === 1 ? "point" : "points"}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>
  {/if}

  <div class="section">
    <h3>Club protection</h3>
    <p class="desc">
      Avoid pairing players from the same club. Off by default. Club names are
      matched case-insensitively; players with no club set are never protected.
    </p>

    <label class="check">
      <input
        type="checkbox"
        checked={clubEnabled}
        disabled={busy}
        onchange={(e) => setClubEnabled(e.currentTarget.checked)}
      />
      Avoid pairing players from the same club
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
          Only for the first
          <input
            type="number"
            min="1"
            step="1"
            class="threshold narrow"
            value={clubRounds ?? 1}
            disabled={busy || clubRounds == null}
            onchange={(e) => editClubRounds(e.currentTarget.value)}
          />
          round(s)
        </label>

        <p class="desc exempt-desc">
          Clubs exempt from protection — their members may still be paired (e.g.
          the host club, whose entrants are expected to meet):
        </p>
        <div class="thresholds">
          {#each exemptClubs as c, i (i)}
            <div class="threshold-row">
              <input
                type="text"
                class="club-input"
                list="known-clubs"
                placeholder="club name"
                value={c}
                disabled={busy}
                onchange={(e) => editExempt(i, e.currentTarget.value)}
              />
              <button
                type="button"
                class="remove"
                disabled={busy}
                title="Remove this exemption"
                onclick={() => removeExempt(i)}>✕</button
              >
            </div>
          {/each}
          {#if exemptClubs.length === 0}
            <p class="muted">No exemptions — every club is protected.</p>
          {/if}
          <button
            type="button"
            class="ghost small"
            disabled={busy}
            onclick={addExempt}>Add exempt club</button
          >
        </div>
        {#if knownClubs.length > 0}
          <datalist id="known-clubs">
            {#each knownClubs as club}<option value={club}></option>{/each}
          </datalist>
        {/if}
      </div>
    {/if}
  </div>

  <div class="section">
    <h3>Floater selection</h3>
    <p class="desc">
      When a score group has to pair across groups, who floats? The weakest of the
      upper group always drops down; this chooses who the lower group sends up.
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
      Classic Swiss — the strongest of the lower group floats up
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
      Median Swiss — the median of the lower group floats up
    </label>
  </div>

  <div class="section">
    <h3>Hybrid cup</h3>
    <p class="desc">
      Run a direct-elimination cup among the top eligible players alongside the
      Swiss (the French / European Championship format). Enabling this adds an
      eligibility column to registration; you pick the bracket size (top 8/16/32/
      64) when you finalize registration.
    </p>
    <label class="check">
      <input
        type="checkbox"
        checked={cupEnabled}
        disabled={busy}
        onchange={(e) => setCupEnabled(e.currentTarget.checked)}
      />
      Hybrid tournament with a direct-elimination cup
    </label>
  </div>

  <div class="section">
    <h3>Handicap games</h3>
    <p class="desc">
      Controls the handicap column(s) in the pairings view. A recommended
      handicap (FFS Annexe 7, from the rating gap) is highlighted in the picker
      whenever it's shown, whichever option is chosen below. Cup games never
      have a handicap.
    </p>
    <label class="check">
      <input
        type="radio"
        name="handicap-policy"
        value="none"
        checked={handicapPolicy === "none"}
        disabled={busy}
        onchange={() => setHandicapPolicy("none")}
      />
      No handicap games — hide the column
    </label>
    <label class="check">
      <input
        type="radio"
        name="handicap-policy"
        value="allowed"
        checked={handicapPolicy === "allowed"}
        disabled={busy}
        onchange={() => setHandicapPolicy("allowed")}
      />
      Handicap games allowed — show the picker
    </label>
    <label class="check">
      <input
        type="radio"
        name="handicap-policy"
        value="suggested"
        checked={handicapPolicy === "suggested"}
        disabled={busy}
        onchange={() => setHandicapPolicy("suggested")}
      />
      Handicap games suggested — also show a suggested-handicap column
    </label>
  </div>
</div>

<style>
  .settings {
    max-width: 32rem;
  }
  h3 {
    margin: 0.4rem 0 0.3rem;
  }
  .section {
    margin-top: 1.75rem;
    border-top: 1px solid #2a2a30;
    padding-top: 1rem;
  }
  .desc {
    color: #9a9aa2;
    font-size: 0.85rem;
    margin: 0 0 1rem;
    line-height: 1.4;
  }
  .thresholds {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-items: flex-start;
  }
  .threshold-row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .prefix {
    color: #c9c9d0;
    font-size: 0.9rem;
  }
  .threshold {
    width: 6rem;
    background: #1c1c22;
    color: inherit;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .threshold.narrow {
    width: 4rem;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.9rem;
    color: #c9c9d0;
  }
  .check input[type="checkbox"] {
    width: 1rem;
    height: 1rem;
  }
  .check + .check {
    margin-top: 0.4rem;
  }
  .club-sub {
    margin: 0.8rem 0 0 1.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .exempt-desc {
    margin: 0;
  }
  .club-input {
    width: 12rem;
    background: #1c1c22;
    color: inherit;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .remove {
    background: transparent;
    border: 1px solid transparent;
    color: #9a9aa2;
    cursor: pointer;
    border-radius: 0.4rem;
    padding: 0.1rem 0.4rem;
  }
  .remove:hover:not(:disabled) {
    color: #f85149;
    border-color: #3a3a42;
  }
  .ghost {
    background: transparent;
    border: 1px solid #3a3a42;
    color: inherit;
    border-radius: 0.4rem;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
    font: inherit;
  }
  .ghost.small {
    font-size: 0.85rem;
  }
  .ghost:hover:not(:disabled) {
    background: #26262c;
  }
  .ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .preview {
    margin-top: 1.25rem;
  }
  .preview h4 {
    margin: 0 0 0.4rem;
    color: #9a9aa2;
    font-size: 0.8rem;
    font-weight: 600;
  }
  .preview ul {
    list-style: none;
    padding: 0;
    margin: 0;
    font-size: 0.9rem;
  }
  .preview li {
    padding: 0.15rem 0;
  }
  .band {
    color: #c9c9d0;
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: #9a9aa2;
    font-size: 0.9rem;
  }
  .hint.warning {
    color: #d29922;
    font-size: 0.85rem;
    margin: 0 0 1rem;
  }
</style>
