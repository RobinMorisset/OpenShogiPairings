<script lang="ts">
  import { untrack } from "svelte";
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type {
    GradeKind,
    HandicapPolicy,
    MacMahonThreshold,
    Player,
    Tiebreak,
    ThresholdCriterion,
    TournamentSettings,
  } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { gradeRank } from "../grade";

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

  // A threshold row being edited: either an ELO value or a dan/kyu grade
  // (only the fields for the active `kind` are meaningful), plus its optional
  // degressive stopping round (null = never drops).
  type ThresholdRow = {
    kind: "elo" | "grade";
    value: number;
    gradeKind: GradeKind;
    gradeLevel: number;
    dropsAfterRound: number | null;
  };

  // A key that sorts ELO thresholds by value, then grade thresholds by
  // strength — mirrors the server's `ThresholdCriterion::sort_key`.
  function criterionSortKey(c: ThresholdCriterion): [number, number] {
    return c.kind === "elo" ? [0, c.value] : [1, gradeRank(c.grade)];
  }

  function criterionEquals(a: ThresholdCriterion, b: ThresholdCriterion): boolean {
    if (a.kind !== b.kind) return false;
    if (a.kind === "elo") return a.value === (b as { kind: "elo"; value: number }).value;
    const bg = (b as { kind: "grade"; grade: { kind: GradeKind; level: number } }).grade;
    return a.grade.kind === bg.kind && a.grade.level === bg.level;
  }

  // Clean, sort and de-duplicate (by criterion) into the server's canonical
  // form — used both to persist and to compare our local rows against what's
  // stored.
  function cleanThresholds(rows: ThresholdRow[]): MacMahonThreshold[] {
    return rows
      .filter((r) =>
        r.kind === "elo"
          ? Number.isFinite(r.value) && r.value >= 1
          : Number.isFinite(r.gradeLevel) && r.gradeLevel >= 1,
      )
      .map((r) => ({
        criterion:
          r.kind === "elo"
            ? ({ kind: "elo", value: Math.round(r.value) } as const)
            : ({
                kind: "grade",
                grade: { kind: r.gradeKind, level: Math.round(r.gradeLevel) },
              } as const),
        drops_after_round:
          r.dropsAfterRound != null && Number.isFinite(r.dropsAfterRound) && r.dropsAfterRound >= 1
            ? Math.round(r.dropsAfterRound)
            : null,
      }))
      .sort((a, b) => {
        const [at, av] = criterionSortKey(a.criterion);
        const [bt, bv] = criterionSortKey(b.criterion);
        return at - bt || av - bv;
      })
      .filter((v, i, arr) => i === 0 || !criterionEquals(v.criterion, arr[i - 1].criterion));
  }

  function eqThresholds(a: MacMahonThreshold[], b: MacMahonThreshold[]): boolean {
    return (
      a.length === b.length &&
      a.every(
        (v, i) =>
          criterionEquals(v.criterion, b[i].criterion) &&
          (v.drops_after_round ?? null) === (b[i].drops_after_round ?? null),
      )
    );
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
  let thresholds = $state<ThresholdRow[]>([]);
  let airtightRounds = $state<number | null>(null);
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
    const sAirtight = settings.airtight_groups_rounds ?? null;
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
        eqThresholds(cleanThresholds(thresholds), sThresholds) &&
        (airtightRounds ?? null) === sAirtight &&
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
        thresholds = sThresholds.map((t) => ({
          kind: t.criterion.kind,
          value: t.criterion.kind === "elo" ? t.criterion.value : 1500,
          gradeKind: t.criterion.kind === "grade" ? t.criterion.grade.kind : "dan",
          gradeLevel: t.criterion.kind === "grade" ? t.criterion.grade.level : 1,
          dropsAfterRound: t.drops_after_round ?? null,
        }));
        airtightRounds = sAirtight;
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
    // MacMahon thresholds, trims/de-dups the exempt clubs).
    onUpdate({
      macmahon_thresholds: cleanThresholds(thresholds),
      airtight_groups_rounds: airtightRounds,
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
    {#each thresholds as row, i (i)}
      <div class="threshold-row">
        <select
          class="threshold-kind"
          value={row.kind}
          disabled={busy}
          onchange={(e) => editThresholdKind(i, e.currentTarget.value as "elo" | "grade")}
        >
          <option value="elo">{$_("settings.thresholdKindElo")}</option>
          <option value="grade">{$_("settings.thresholdKindGrade")}</option>
        </select>
        {#if row.kind === "elo"}
          <input
            type="number"
            min="1"
            step="1"
            class="threshold"
            value={row.value}
            disabled={busy}
            onchange={(e) => editThresholdValue(i, e.currentTarget.value)}
          />
        {:else}
          <input
            type="number"
            min="1"
            step="1"
            class="threshold narrow"
            value={row.gradeLevel}
            disabled={busy}
            onchange={(e) => editThresholdGradeLevel(i, e.currentTarget.value)}
          />
          <select
            class="threshold-kind"
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
            class="threshold narrow"
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
            <strong>
              {$_(b.points === 1 ? "settings.pointsValueSingular" : "settings.pointsValuePlural", { values: { points: b.points } })}
            </strong>
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

  {#if thresholds.length > 0}
    <div class="section">
      <h3>{$_("settings.airtightGroupsTitle")}</h3>
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
          class="threshold narrow"
          value={airtightRounds ?? 1}
          disabled={busy || airtightRounds == null}
          onchange={(e) => editAirtightRounds(e.currentTarget.value)}
        />
        {$_("settings.onlyFirstRoundsSuffix")}
      </label>
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
  .threshold-kind {
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
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
  .drop-check {
    white-space: nowrap;
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
