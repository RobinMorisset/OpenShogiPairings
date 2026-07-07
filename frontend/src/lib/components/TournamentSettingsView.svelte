<script lang="ts">
  import { untrack } from "svelte";
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type { HandicapPolicy, Player, Tiebreak, TournamentSettings } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";

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

  // Distinct club names among the players (first spelling kept) with their
  // player count, for the exempt datalist — sorted by decreasing count (ties
  // broken alphabetically) so the clubs most worth exempting sort first.
  const knownClubs = $derived.by(() => {
    const counts = new Map<string, { name: string; count: number }>();
    for (const p of players) {
      const name = p.club?.trim();
      if (!name) continue;
      const key = name.toLowerCase();
      const existing = counts.get(key);
      if (existing) existing.count++;
      else counts.set(key, { name, count: 1 });
    }
    return [...counts.values()].sort(
      (a, b) => b.count - a.count || a.name.localeCompare(b.name),
    );
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
  let tiebreaks = $state<Tiebreak[]>([]);
  let eloEnabled = $state(false);
  let eloKPercent = $state(100);
  let eloProvisionalPercent = $state(200);

  // In the experimental ELO mode the Swiss knobs (MacMahon, degressive, club
  // protection, floater selection) don't apply, so they're greyed out.
  const swissDisabled = $derived(eloEnabled);

  // Metrics not yet in the ranking order — the choices for the "add" dropdown.
  // Estimated ELO is only meaningful in ELO pairing mode (otherwise it just sits
  // at the registration rating), so it isn't offered while that mode is off.
  const availableTiebreaks = $derived(
    TIEBREAKS.filter(
      (t) => !tiebreaks.includes(t.code) && (eloEnabled || t.code !== "est_elo"),
    ),
  );
  const labelOf = (code: Tiebreak) => tiebreakLabel(code, $_);
  const titleOf = (code: Tiebreak) => tiebreakTitle(code, $_);

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
    const sTiebreaks = settings.tiebreaks ?? [];
    const sElo = settings.elo_pairing_enabled ?? false;
    const sEloK = settings.elo_k_multiplier_percent ?? 100;
    const sEloProv = settings.elo_provisional_multiplier_percent ?? 200;
    untrack(() => {
      const matches =
        eq(cleanSorted(thresholds), sThresholds) &&
        eq(cleanSorted(removals), sRemovals) &&
        clubEnabled === sEnabled &&
        (clubRounds ?? null) === sRounds &&
        eqStr(normExempt(exemptClubs), sExempt) &&
        floaterStyle === sFloater &&
        cupEnabled === sCup &&
        handicapPolicy === sHandicap &&
        eqStr(tiebreaks, sTiebreaks) &&
        eloEnabled === sElo &&
        eloKPercent === sEloK &&
        eloProvisionalPercent === sEloProv;
      if (!matches) {
        thresholds = [...sThresholds];
        removals = [...sRemovals];
        clubEnabled = sEnabled;
        clubRounds = sRounds;
        exemptClubs = [...sExempt];
        floaterStyle = sFloater;
        cupEnabled = sCup;
        handicapPolicy = sHandicap;
        tiebreaks = [...sTiebreaks];
        eloEnabled = sElo;
        eloKPercent = sEloK;
        eloProvisionalPercent = sEloProv;
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
      tiebreaks: [...tiebreaks],
      elo_pairing_enabled: eloEnabled,
      elo_k_multiplier_percent: eloKPercent,
      elo_provisional_multiplier_percent: eloProvisionalPercent,
    });
  }

  function setEloEnabled(v: boolean) {
    eloEnabled = v;
    // Estimated ELO isn't a valid ranking criterion without this mode, so drop
    // it from the order when turning the mode off (mirrors the server).
    if (!v) tiebreaks = tiebreaks.filter((c) => c !== "est_elo");
    persist();
  }

  function editEloMultiplier(raw: string) {
    // Presented as a decimal multiplier (×1.0), stored as an integer percent.
    const m = Number(raw);
    const pct = Math.round((Number.isFinite(m) ? m : 1) * 100);
    eloKPercent = Math.max(1, pct);
    persist();
  }

  function editEloProvisionalMultiplier(raw: string) {
    // Decimal multiplier stored as an integer percent; never below ×1 (a
    // provisional rating shouldn't be treated as more reliable than an established
    // one — the server clamps this too).
    const m = Number(raw);
    const pct = Math.round((Number.isFinite(m) ? m : 2) * 100);
    eloProvisionalPercent = Math.max(100, pct);
    persist();
  }

  function addTiebreak(code: Tiebreak) {
    if (!code || tiebreaks.includes(code)) return;
    tiebreaks.push(code);
    persist();
  }

  function removeTiebreak(i: number) {
    tiebreaks.splice(i, 1);
    persist();
  }

  function moveTiebreak(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= tiebreaks.length) return;
    [tiebreaks[i], tiebreaks[j]] = [tiebreaks[j], tiebreaks[i]];
    persist();
  }

  // Drag-and-drop reordering — an alternative to the ▲▼ buttons above, not a
  // replacement (buttons stay for keyboard/accessibility).
  let dragIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);

  function reorderTiebreak(from: number, to: number) {
    if (from === to) return;
    const [item] = tiebreaks.splice(from, 1);
    tiebreaks.splice(to, 0, item);
    persist();
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

  // How many registered players fall into each MacMahon band — unrated
  // players and those below the first threshold count as band 0, mirroring
  // the server's own point calculation (one point per threshold reached).
  const bandPlayerCounts = $derived.by(() => {
    const t = normThresholds;
    const counts = new Array(t.length + 1).fill(0);
    for (const p of players) {
      let band = 0;
      if (p.rating != null) {
        for (const threshold of t) {
          if (p.rating >= threshold) band++;
          else break;
        }
      }
      counts[band]++;
    }
    return counts;
  });

  // Preview of the resulting bands, e.g. "below 1200 → 0".
  const bands = $derived.by(() => {
    const t = normThresholds;
    const rows: { label: string; points: number; count: number }[] = [];
    if (t.length === 0) return rows;
    const counts = bandPlayerCounts;
    rows.push({
      label: $_("settings.bandBelow", { values: { value: t[0] } }),
      points: 0,
      count: counts[0],
    });
    for (let i = 0; i < t.length; i++) {
      const hi = t[i + 1];
      rows.push({
        label:
          hi != null
            ? $_("settings.bandRange", { values: { lo: t[i], hi: hi - 1 } })
            : $_("settings.bandAndAbove", { values: { value: t[i] } }),
        points: i + 1,
        count: counts[i + 1],
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
      rows.push({
        label:
          start === r
            ? $_("settings.roundLabel", { values: { n: r } })
            : $_("settings.roundsRangeLabel", { values: { from: start, to: r } }),
        max: active,
      });
      active = Math.max(0, active - (counts.get(r) ?? 0));
      start = r + 1;
    }
    rows.push({ label: $_("settings.roundPlus", { values: { n: start } }), max: active });
    return rows;
  });
</script>

<div class="settings">
  {#if finalized}
    <p class="hint warning">
      ⚠ {$_("settings.finalizedWarning")}
    </p>
  {/if}

  <div class="section mode-section">
    <h3>{$_("settings.eloModeTitle")}</h3>
    <p class="desc">
      {$_("settings.eloModeDesc")}
    </p>
    <label class="check">
      <input
        type="checkbox"
        checked={eloEnabled}
        disabled={busy}
        onchange={(e) => setEloEnabled(e.currentTarget.checked)}
      />
      {$_("settings.eloModeCheckbox")}
    </label>
    {#if eloEnabled}
      <label class="check elo-k">
        {$_("settings.eloDriftMultiplier")}
        <input
          type="number"
          min="0.5"
          step="0.5"
          class="threshold narrow"
          value={eloKPercent / 100}
          disabled={busy}
          onchange={(e) => editEloMultiplier(e.currentTarget.value)}
        />
      </label>
      <p class="desc small-note">
        {$_("settings.eloDriftDesc")}
      </p>
      <label class="check elo-k">
        {$_("settings.eloProvisionalMultiplier")}
        <input
          type="number"
          min="1"
          step="0.5"
          class="threshold narrow"
          value={eloProvisionalPercent / 100}
          disabled={busy}
          onchange={(e) => editEloProvisionalMultiplier(e.currentTarget.value)}
        />
      </label>
      <p class="desc small-note">
        {$_("settings.eloProvisionalDesc")}
      </p>
    {/if}
  </div>

  <fieldset class="swiss-fieldset" disabled={swissDisabled}>
    <h3>{$_("settings.macmahonTitle")}</h3>
    <p class="desc">
    {$_("settings.macmahonDesc")}
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
          title={$_("settings.removeThreshold")}
          onclick={() => removeThreshold(i)}>✕</button
        >
      </div>
    {/each}
    {#if thresholds.length === 0}
      <p class="muted">{$_("settings.noThresholds")}</p>
    {/if}
    <button
      type="button"
      class="ghost small"
      disabled={busy}
      onclick={addThreshold}>{$_("settings.addThreshold")}</button
    >
  </div>

  {#if bands.length > 0}
    <div class="preview">
      <h4>{$_("settings.startingPoints")}</h4>
      <ul>
        {#each bands as b (b.points)}
          <li>
            <span class="band">{b.label}</span> → <strong>{b.points}</strong>
            <span class="band-count">
              ({$_(b.count === 1 ? "settings.playerCountSingular" : "settings.playerCountPlural", { values: { count: b.count } })})
            </span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if thresholds.length > 0}
    <div class="section">
      <h3>{$_("settings.degressiveTitle")}</h3>
      <p class="desc">
        {$_("settings.degressiveDesc")}
      </p>

      <div class="thresholds">
        {#each removals as r, i (i)}
          <div class="threshold-row">
            <span class="prefix">{$_("settings.dropPrefix")}</span>
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
              title={$_("settings.removeDrop")}
              onclick={() => removeRemoval(i)}>✕</button
            >
          </div>
        {/each}
        {#if removals.length === 0}
          <p class="muted">{$_("settings.noDrops")}</p>
        {/if}
        <button
          type="button"
          class="ghost small"
          disabled={busy || !canAddRemoval}
          title={canAddRemoval ? "" : $_("settings.cantDropMore")}
          onclick={addRemoval}>{$_("settings.addDrop")}</button
        >
      </div>

      {#if schedule.length > 0}
        <div class="preview">
          <h4>{$_("settings.spreadOverRounds")}</h4>
          <ul>
            {#each schedule as s (s.label)}
              <li>
                <span class="band">{s.label}</span> → {$_("settings.upTo")}
                <strong>{s.max}</strong>
                {$_(s.max === 1 ? "settings.startingPointSingular" : "settings.startingPointsPlural")}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>
  {/if}

  <div class="section">
    <h3>{$_("settings.clubProtectionTitle")}</h3>
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
            class="threshold narrow"
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
                class="club-input"
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
            class="ghost small"
            disabled={busy}
            onclick={addExempt}>{$_("settings.addExemptClub")}</button
          >
        </div>
        {#if knownClubs.length > 0}
          <datalist id="known-clubs">
            {#each knownClubs as club (club.name)}
              <option
                value={club.name}
                label={`${club.name} (${club.count})`}
              >
                {club.name} ({club.count})
              </option>
            {/each}
          </datalist>
        {/if}
      </div>
    {/if}
  </div>

  <div class="section">
    <h3>{$_("settings.floaterTitle")}</h3>
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
  </div>
  </fieldset>

  <div class="section">
    <h3>{$_("settings.hybridCupTitle")}</h3>
    <p class="desc">
      {$_("settings.hybridCupDesc")}
    </p>
    <label class="check">
      <input
        type="checkbox"
        checked={cupEnabled}
        disabled={busy}
        onchange={(e) => setCupEnabled(e.currentTarget.checked)}
      />
      {$_("settings.hybridCupCheckbox")}
    </label>
  </div>

  <div class="section">
    <h3>{$_("settings.handicapTitle")}</h3>
    <p class="desc">
      {$_("settings.handicapDesc")}
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
      {$_("settings.handicapNone")}
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
      {$_("settings.handicapAllowed")}
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
      {$_("settings.handicapSuggested")}
    </label>
  </div>

  <div class="section">
    <h3>{$_("settings.rankingTitle")}</h3>
    <p class="desc">
      {$_("settings.rankingDesc")}
    </p>

    <div class="thresholds">
      {#each tiebreaks as code, i (code)}
        <div
          class="threshold-row"
          class:dragging={dragIndex === i}
          class:drag-over={dragOverIndex === i && dragIndex !== null && dragIndex !== i}
          role="listitem"
          draggable={!busy}
          ondragstart={(e) => {
            dragIndex = i;
            e.dataTransfer?.setData("text/plain", String(i));
            if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
          }}
          ondragover={(e) => {
            e.preventDefault();
            dragOverIndex = i;
          }}
          ondragleave={() => {
            if (dragOverIndex === i) dragOverIndex = null;
          }}
          ondrop={(e) => {
            e.preventDefault();
            if (dragIndex !== null) reorderTiebreak(dragIndex, i);
            dragIndex = null;
            dragOverIndex = null;
          }}
          ondragend={() => {
            dragIndex = null;
            dragOverIndex = null;
          }}
        >
          <span class="drag-handle" title={$_("settings.dragToReorder")} aria-hidden="true"
            >⠿</span
          >
          <span class="tb-rank">{i + 1}.</span>
          <span class="tb-label" title={titleOf(code)}>{labelOf(code)}</span>
          <button
            type="button"
            class="remove"
            disabled={busy || i === 0}
            title={$_("settings.moveUp")}
            onclick={() => moveTiebreak(i, -1)}>▲</button
          >
          <button
            type="button"
            class="remove"
            disabled={busy || i === tiebreaks.length - 1}
            title={$_("settings.moveDown")}
            onclick={() => moveTiebreak(i, 1)}>▼</button
          >
          <button
            type="button"
            class="remove"
            disabled={busy}
            title={$_("settings.removeTiebreak")}
            onclick={() => removeTiebreak(i)}>✕</button
          >
        </div>
      {/each}
      {#if tiebreaks.length === 0}
        <p class="muted">
          {$_("settings.noTiebreaks")}
        </p>
      {/if}
      {#if availableTiebreaks.length > 0}
        <div class="threshold-row">
          <select
            class="tb-select"
            disabled={busy}
            value=""
            onchange={(e) => {
              addTiebreak(e.currentTarget.value as Tiebreak);
              e.currentTarget.value = "";
            }}
          >
            <option value="" disabled>{$_("settings.addTiebreakPlaceholder")}</option>
            {#each availableTiebreaks as t (t.code)}
              <option value={t.code} title={tiebreakTitle(t.code, $_)}
                >{tiebreakLabel(t.code, $_)} — {tiebreakTitle(t.code, $_)}</option
              >
            {/each}
          </select>
        </div>
      {/if}
    </div>
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
    border-top: 1px solid var(--border-divider);
    padding-top: 1rem;
  }
  /* Groups the Swiss-only sections so they can be greyed out as one in ELO mode.
     Reset the browser's default fieldset chrome; the inner `.section`s keep their
     own separators. */
  fieldset.swiss-fieldset {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  fieldset.swiss-fieldset:disabled {
    opacity: 0.5;
  }
  .mode-section {
    margin-top: 0.5rem;
    border-top: none;
    padding-top: 0;
  }
  .elo-k {
    margin-top: 0.7rem;
  }
  .small-note {
    margin: 0.5rem 0 0;
  }
  .desc {
    color: var(--text-secondary);
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
  .threshold-row.dragging {
    opacity: 0.4;
  }
  .threshold-row.drag-over {
    box-shadow: 0 -2px 0 0 var(--border-accent-strong);
  }
  .drag-handle {
    cursor: grab;
    color: var(--text-muted);
    font-size: 0.9rem;
    line-height: 1;
  }
  .prefix {
    color: var(--text-strong);
    font-size: 0.9rem;
  }
  .threshold {
    width: 6rem;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
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
    color: var(--text-strong);
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
  .tb-rank {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    width: 1.4rem;
    text-align: right;
  }
  .tb-label {
    min-width: 5.5rem;
    font-weight: 600;
    color: var(--text-strong);
  }
  .tb-select {
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .club-input {
    width: 12rem;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .remove {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-secondary);
    cursor: pointer;
    border-radius: 0.4rem;
    padding: 0.1rem 0.4rem;
  }
  .remove:hover:not(:disabled) {
    color: var(--color-danger);
    border-color: var(--border-soft);
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border-soft);
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
    background: var(--bg-hover);
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
    color: var(--text-secondary);
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
    color: var(--text-strong);
    font-variant-numeric: tabular-nums;
  }
  .band-count {
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  .muted {
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  .hint.warning {
    color: var(--color-warning);
    font-size: 0.85rem;
    margin: 0 0 1rem;
  }
</style>
