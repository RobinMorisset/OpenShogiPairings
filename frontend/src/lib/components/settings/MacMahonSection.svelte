<!-- The MacMahon thresholds, what they produce, and the two settings that hang
     off them: estimate-based starting points, and airtight groups. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { GradeKind, Player, ThresholdCriterion } from "../../types";
  import { gradeRank } from "../../grade";
  import { cleanThresholds, type ThresholdRow } from "../../thresholds";
  import EstimatorKnobs from "./EstimatorKnobs.svelte";

  interface Props {
    /** Local editable rows, in *entry* order (not sorted) so the row a referee is
     *  editing never jumps or shows a stale value. */
    thresholds: ThresholdRow[];
    /** Rounds the airtight-groups rule applies to, `null` when it is off. */
    airtightRounds: number | null;
    /** A team has an average rating, not a grade, so grade thresholds are out. */
    teamMode: boolean;
    /** The registered players, whose starting points the previews count. */
    players: Player[];
    macmahonFromElo: boolean;
    /** Whether estimate-based MacMahon can apply (it needs an ELO threshold). */
    hasEloThreshold: boolean;
    /** Whether to offer it at all — kept on screen when set but inert. */
    showMacmahonFromElo: boolean;
    eloApplyTo: "unrated" | "all";
    unratedPrior: "flat" | "laplace";
    busy: boolean;
    /** Turning estimate-based MacMahon on also seeds the estimator, which is the
     *  parent's to do. */
    setMacmahonFromElo: (on: boolean) => void;
    setEloApplyTo: (v: "unrated" | "all") => void;
    setUnratedPrior: (v: "flat" | "laplace") => void;
    persist: () => void;
  }

  let {
    thresholds = $bindable(),
    airtightRounds = $bindable(),
    teamMode,
    players,
    macmahonFromElo,
    hasEloThreshold,
    showMacmahonFromElo,
    eloApplyTo,
    unratedPrior,
    busy,
    setMacmahonFromElo,
    setEloApplyTo,
    setUnratedPrior,
    persist,
  }: Props = $props();

  function addThreshold() {
    const eloValues = thresholds.filter((t) => t.kind === "elo").map((t) => t.value);
    const maxValue = eloValues.length ? Math.max(...eloValues) : 1400;
    thresholds.push({
      kind: "elo",
      value: maxValue + 100,
      gradeKind: "dan",
      gradeLevel: 1,
      dropsAfterRound: null,
    });
    persist();
  }

  function removeThreshold(i: number) {
    thresholds.splice(i, 1);
    persist();
  }

  function editThresholdKind(i: number, kind: "elo" | "grade") {
    thresholds[i].kind = kind;
    persist();
  }

  function editThresholdValue(i: number, raw: string) {
    thresholds[i].value = Number(raw);
    persist();
  }

  function editThresholdGradeLevel(i: number, raw: string) {
    thresholds[i].gradeLevel = Number(raw);
    persist();
  }

  function editThresholdGradeKind(i: number, kind: GradeKind) {
    thresholds[i].gradeKind = kind;
    persist();
  }

  function toggleThresholdDrop(i: number, on: boolean) {
    thresholds[i].dropsAfterRound = on ? (thresholds[i].dropsAfterRound ?? 1) : null;
    persist();
  }

  function editThresholdDropRound(i: number, raw: string) {
    const n = Math.round(Number(raw));
    thresholds[i].dropsAfterRound = Number.isFinite(n) && n >= 1 ? n : 1;
    persist();
  }

  function setAirtightEnabled(on: boolean) {
    airtightRounds = on ? (airtightRounds ?? 1) : null;
    persist();
  }

  function editAirtightRounds(raw: string) {
    const n = Math.round(Number(raw));
    airtightRounds = Number.isFinite(n) && n >= 1 ? n : 1;
    persist();
  }

  // Normalized (sorted, de-duplicated) view of the local rows — drives the
  // previews so they always reflect what the server will store, updated live on
  // each edit rather than lagging a round-trip.
  const normalized = $derived(cleanThresholds(thresholds));

  // Whether to offer airtight groups: the rule forbids pairing across MacMahon
  // groups, and with no threshold there is only one group, so it has nothing to
  // forbid. Mirrors the server, which clears the window in `normalized()` when
  // the last threshold goes — so unlike estimate-based MacMahon there is no
  // set-but-inert state to keep on screen.
  const showAirtight = $derived(normalized.length > 0);

  // Whether a player meets one threshold, mirroring the server's
  // `ThresholdCriterion::met_by`: a missing rating/grade never meets the
  // corresponding kind of threshold.
  function meetsCriterion(c: ThresholdCriterion, p: Player): boolean {
    if (c.kind === "elo") return p.rating != null && p.rating >= c.value;
    return p.grade != null && gradeRank(p.grade) >= gradeRank(c.grade);
  }

  // A player's MacMahon starting points: the number of normalized thresholds
  // they meet. ELO and grade thresholds are independent axes, so — unlike a
  // single-axis ELO ladder — meeting one doesn't imply meeting any other in
  // particular; only the *count* is well-defined, not a contiguous "band".
  function playerPoints(p: Player): number {
    return normalized.filter((t) => meetsCriterion(t.criterion, p)).length;
  }

  // How many registered players land on each starting-points value, 0..total.
  const bandPlayerCounts = $derived.by(() => {
    const counts = new Array(normalized.length + 1).fill(0);
    for (const p of players) counts[playerPoints(p)]++;
    return counts;
  });

  // Preview of the resulting starting-points histogram.
  const bands = $derived.by(() => {
    if (normalized.length === 0) return [];
    return bandPlayerCounts.map((count, points) => ({ points, count }));
  });

  // Preview of how the starting-point spread shrinks over the rounds, driven by
  // each threshold's own drop round.
  const schedule = $derived.by(() => {
    const total = normalized.length;
    const rem = normalized
      .map((t) => t.drops_after_round)
      .filter((r): r is number => r != null);
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

<section class="section">
  <h3>{$_("settings.macmahonTitle")}</h3>
  <p class="desc">
    {$_("settings.macmahonDesc")}
  </p>

  <div class="grid macmahon-grid">
    <div>
      <div class="thresholds">
        {#each thresholds as row, i (i)}
          <div class="threshold-row">
            <select
              class="threshold-kind control-sm control-quiet"
              value={row.kind}
              disabled={busy}
              onchange={(e) => editThresholdKind(i, e.currentTarget.value as "elo" | "grade")}
            >
              <option value="elo">{$_("settings.thresholdKindElo")}</option>
              <!-- A team has an average rating, not a grade, so the server
                   rejects grade thresholds in team mode. -->
              {#if !teamMode}
                <option value="grade">{$_("settings.thresholdKindGrade")}</option>
              {/if}
            </select>
            {#if row.kind === "elo"}
              <input
                type="number"
                min="1"
                step="1"
                class="threshold control-sm control-quiet"
                value={row.value}
                disabled={busy}
                onchange={(e) => editThresholdValue(i, e.currentTarget.value)}
              />
            {:else}
              <input
                type="number"
                min="1"
                step="1"
                class="threshold narrow control-sm control-quiet"
                value={row.gradeLevel}
                disabled={busy}
                onchange={(e) => editThresholdGradeLevel(i, e.currentTarget.value)}
              />
              <select
                class="threshold-kind control-sm control-quiet"
                value={row.gradeKind}
                disabled={busy}
                onchange={(e) => editThresholdGradeKind(i, e.currentTarget.value as GradeKind)}
              >
                <option value="dan">{$_("settings.gradeKindDan")}</option>
                <option value="kyu">{$_("settings.gradeKindKyu")}</option>
              </select>
            {/if}
            <label class="check drop-check">
              <input
                type="checkbox"
                checked={row.dropsAfterRound != null}
                disabled={busy}
                onchange={(e) => toggleThresholdDrop(i, e.currentTarget.checked)}
              />
              {$_("settings.dropAfterRoundCheckbox")}
              <input
                type="number"
                min="1"
                step="1"
                class="threshold narrow control-sm control-quiet"
                value={row.dropsAfterRound ?? 1}
                disabled={busy || row.dropsAfterRound == null}
                onchange={(e) => editThresholdDropRound(i, e.currentTarget.value)}
              />
            </label>
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
          class="ghost control-xs control-quiet"
          disabled={busy}
          onclick={addThreshold}>{$_("settings.addThreshold")}</button
        >
      </div>
      <!-- Each preview stays in the same column as the control it previews:
           the starting points are what the thresholds above produce, and the
           spread over rounds is what their "stops after round" boxes do. -->
      {#if bands.length > 0}
        <div class="preview">
          <h4>{$_("settings.startingPoints")}</h4>
          <ul>
            {#each bands as b (b.points)}
              <li>
                <strong>
                  {$_("settings.pointsValue", { values: { points: b.points } })}
                </strong>
                <span class="band-count">
                  ({$_("settings.playerCount", { values: { count: b.count } })})
                </span>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
      <!-- The accelerated / degressive Swiss has no control of its own: it is
           the per-threshold "stops after round" checkbox above. -->
      <p class="desc small-note">
        {$_("settings.degressiveDesc")}
      </p>
      {#if schedule.length > 0}
        <div class="preview">
          <h4>{$_("settings.spreadOverRounds")}</h4>
          <ul>
            {#each schedule as s (s.label)}
              <li>
                <span class="band">{s.label}</span> → {$_("settings.upTo")}
                <strong>{s.max}</strong>
                {$_("settings.startingPointCount", { values: { max: s.max } })}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>

    {#if showMacmahonFromElo}
      <fieldset class="sub">
        <legend>{$_("settings.macmahonFromEloTitle")}</legend>
        <p class="desc">
          {$_("settings.macmahonFromEloDesc")}
        </p>
        <label class="check">
          <input
            type="checkbox"
            checked={macmahonFromElo}
            disabled={busy || !hasEloThreshold}
            onchange={(e) => setMacmahonFromElo(e.currentTarget.checked)}
          />
          {$_("settings.macmahonFromEloCheckbox")}
        </label>
        {#if !hasEloThreshold}
          <p class="hint muted">{$_("settings.macmahonFromEloNeedsEloThreshold")}</p>
        {/if}
        {#if macmahonFromElo}
          <EstimatorKnobs
            {eloApplyTo}
            {unratedPrior}
            {busy}
            {setEloApplyTo}
            {setUnratedPrior}
          />
        {/if}
      </fieldset>
    {/if}

    {#if showAirtight}
      <fieldset class="sub">
        <legend>{$_("settings.airtightGroupsTitle")}</legend>
        <p class="desc">
          {$_("settings.airtightGroupsDesc")}
        </p>
        <label class="check">
          <input
            type="checkbox"
            checked={airtightRounds != null}
            disabled={busy}
            onchange={(e) => setAirtightEnabled(e.currentTarget.checked)}
          />
          {$_("settings.onlyFirstRoundsPrefix")}
          <input
            type="number"
            min="1"
            step="1"
            class="threshold narrow control-sm control-quiet"
            value={airtightRounds ?? 1}
            disabled={busy || airtightRounds == null}
            onchange={(e) => editAirtightRounds(e.currentTarget.value)}
          />
          {$_("settings.onlyFirstRoundsSuffix")}
        </label>
      </fieldset>
    {/if}
  </div>
</section>

<style>
  .macmahon-grid {
    --col-min: 26rem;
  }
  .drop-check {
    white-space: nowrap;
  }
  .preview {
    margin-top: 0.5rem;
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
</style>
