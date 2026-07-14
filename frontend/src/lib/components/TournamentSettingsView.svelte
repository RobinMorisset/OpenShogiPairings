<script lang="ts">
  import { untrack } from "svelte";
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type {
    EloPriorShape,
    GradeKind,
    HandicapPolicy,
    MacMahonThreshold,
    Player,
    Tiebreak,
    ThresholdCriterion,
    TournamentSettings,
  } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { saveSettings } from "../tournamentFile";
  import { gradeRank } from "../grade";
  import { cleanThresholds, eqThresholds, normExempt, type ThresholdRow } from "../thresholds";

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

  // Download the current settings as JSON (for the simulation CLI's --configs,
  // or to share a configuration). Fire-and-forget: a cancelled dialog is a no-op.
  function exportSettings() {
    void saveSettings("tournament", settings);
  }

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

  function eqStr(a: string[], b: string[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
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
  let handicapWielRule = $state(false);
  let halfPointAbsences = $state(false);
  let tiebreaks = $state<Tiebreak[]>([]);
  let eloEnabled = $state(false);
  let macmahonFromElo = $state(false);
  let eloKPercent = $state(100);
  let eloProvisionalPercent = $state(200);
  let eloUnratedCenter = $state(600);
  let eloUnratedK = $state(705);
  let eloPriorShape = $state<EloPriorShape>("gaussian");
  let eloLoosenessEstablishedPercent = $state(100);
  let eloLoosenessProvisionalPercent = $state(100);
  let eloLoosenessUnratedPercent = $state(100);

  // Whether the "MacMahon from estimated ELO" option can apply: it compares the
  // estimate against ELO thresholds, so it's inert (and greyed out) with only
  // grade thresholds, or none. Mirrors the server's `macmahon_from_estimate_active`.
  const hasEloThreshold = $derived(thresholds.some((t) => t.kind === "elo"));

  // Two-way pairing mode (Swiss or experimental pure ELO), for the radio group.
  const pairingMode = $derived(eloEnabled ? "elo" : "swiss");

  // In the experimental (pure) ELO mode the Swiss knobs (MacMahon, degressive,
  // club protection, floater selection) don't apply, so they're greyed out; the
  // floater-selection rule in particular is replaced by the ELO-gap rule.
  const swissDisabled = $derived(eloEnabled);
  const floaterDisabled = $derived(eloEnabled);

  // Whether a live ELO estimate is maintained, so the estimated-ELO tie-break is
  // meaningful: the ELO pairing mode, or estimate-based MacMahon with an ELO
  // threshold to compare against. Mirrors the server's normalization gate.
  const eloEstimateLive = $derived(
    eloEnabled || (macmahonFromElo && hasEloThreshold),
  );

  // The two simplified estimator controls (the only ELO knobs a referee sees).
  // "Apply estimates to": k = 0 means only unrated players are estimated (rated
  // players keep their registration rating); any positive k estimates everyone.
  const eloApplyTo = $derived<"unrated" | "all">(
    eloKPercent === 0 ? "unrated" : "all",
  );
  // "Unrated prior": the flat performance rating (default) or the tuned Laplace.
  // Anything not explicitly Laplace reads as the flat default.
  const unratedPrior = $derived<"flat" | "laplace">(
    eloPriorShape === "laplace" ? "laplace" : "flat",
  );

  // Metrics not yet in the ranking order — the choices for the "add" dropdown.
  // Estimated ELO is only meaningful when a live estimate is maintained
  // (otherwise it just sits at the registration rating), so it isn't offered
  // otherwise.
  const availableTiebreaks = $derived(
    TIEBREAKS.filter(
      (t) => !tiebreaks.includes(t.code) && (eloEstimateLive || t.code !== "est_elo"),
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
    const sHandicapWiel = settings.handicap_wiel_rule;
    const sHalfPointAbsences = settings.half_point_absences ?? false;
    const sTiebreaks = settings.tiebreaks ?? [];
    const sElo = settings.elo_pairing_enabled ?? false;
    const sMacmahonFromElo = settings.macmahon_from_estimated_elo ?? false;
    const sEloK = settings.elo_k_multiplier_percent ?? 100;
    const sEloProv = settings.elo_provisional_multiplier_percent ?? 200;
    const sEloUnratedCenter = settings.elo_unrated_prior_center ?? 600;
    const sEloUnratedK = settings.elo_unrated_k ?? 705;
    const sEloPriorShape = settings.elo_prior_shape_unrated ?? "gaussian";
    const sEloLooseEst = settings.elo_upward_looseness_established_percent ?? 100;
    const sEloLooseProv = settings.elo_upward_looseness_provisional_percent ?? 100;
    const sEloLooseUnr = settings.elo_upward_looseness_unrated_percent ?? 100;
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
        handicapWielRule === sHandicapWiel &&
        halfPointAbsences === sHalfPointAbsences &&
        eqStr(tiebreaks, sTiebreaks) &&
        eloEnabled === sElo &&
        macmahonFromElo === sMacmahonFromElo &&
        eloKPercent === sEloK &&
        eloProvisionalPercent === sEloProv &&
        eloUnratedCenter === sEloUnratedCenter &&
        eloUnratedK === sEloUnratedK &&
        eloPriorShape === sEloPriorShape &&
        eloLoosenessEstablishedPercent === sEloLooseEst &&
        eloLoosenessProvisionalPercent === sEloLooseProv &&
        eloLoosenessUnratedPercent === sEloLooseUnr;
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
        handicapWielRule = sHandicapWiel;
        halfPointAbsences = sHalfPointAbsences;
        tiebreaks = [...sTiebreaks];
        eloEnabled = sElo;
        macmahonFromElo = sMacmahonFromElo;
        eloKPercent = sEloK;
        eloProvisionalPercent = sEloProv;
        eloUnratedCenter = sEloUnratedCenter;
        eloUnratedK = sEloUnratedK;
        eloPriorShape = sEloPriorShape;
        eloLoosenessEstablishedPercent = sEloLooseEst;
        eloLoosenessProvisionalPercent = sEloLooseProv;
        eloLoosenessUnratedPercent = sEloLooseUnr;
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
      handicap_wiel_rule: handicapWielRule,
      half_point_absences: halfPointAbsences,
      tiebreaks: [...tiebreaks],
      elo_pairing_enabled: eloEnabled,
      macmahon_from_estimated_elo: macmahonFromElo,
      elo_k_multiplier_percent: eloKPercent,
      elo_provisional_multiplier_percent: eloProvisionalPercent,
      elo_unrated_prior_center: eloUnratedCenter,
      elo_unrated_k: eloUnratedK,
      elo_prior_shape_established: "gaussian",
      elo_prior_shape_provisional: "gaussian",
      elo_prior_shape_unrated: eloPriorShape,
      elo_upward_looseness_established_percent: eloLoosenessEstablishedPercent,
      elo_upward_looseness_provisional_percent: eloLoosenessProvisionalPercent,
      elo_upward_looseness_unrated_percent: eloLoosenessUnratedPercent,
    });
  }

  // Sane estimator defaults for a referee first turning on a live-estimate mode:
  // estimate unrated players only (k = 0) with the flat performance-rating prior.
  // Only applied when the estimator was never configured via the two controls (the
  // unrated shape is still the struct default "gaussian", which those controls
  // don't offer), so it never clobbers an explicit flat/Laplace choice.
  function applyEstimatorDefaultsIfUnset() {
    if (eloPriorShape !== "flat" && eloPriorShape !== "laplace") {
      eloPriorShape = "flat";
      eloKPercent = 0;
    }
  }

  function setPairingMode(mode: "swiss" | "elo") {
    eloEnabled = mode === "elo";
    if (eloEnabled) applyEstimatorDefaultsIfUnset();
    // Estimated ELO is only a valid ranking criterion while a live estimate is
    // maintained; drop it from the order otherwise (mirrors the server).
    if (!eloEstimateLive) tiebreaks = tiebreaks.filter((c) => c !== "est_elo");
    persist();
  }

  function setMacmahonFromElo(on: boolean) {
    macmahonFromElo = on;
    if (on) applyEstimatorDefaultsIfUnset();
    // Turning it off (in plain Swiss) may leave no live estimate, so the
    // estimated-ELO tie-break is no longer valid — mirror the server and drop it.
    if (!eloEstimateLive) tiebreaks = tiebreaks.filter((c) => c !== "est_elo");
    persist();
  }

  // Control 1 — who the estimate applies to. k = 0 pins rated players to their
  // registration rating (estimate unrated only); k = 100% estimates everyone.
  function setEloApplyTo(v: "unrated" | "all") {
    eloKPercent = v === "unrated" ? 0 : 100;
    persist();
  }

  // Control 2 — the unrated prior. "flat" is the improper performance rating;
  // "laplace" switches to the tuned asymmetric Huber-Laplace (centre 700, K 260,
  // upward looseness ×3) — the tuned knobs are set here rather than shown.
  function setUnratedPrior(v: "flat" | "laplace") {
    if (v === "laplace") {
      eloPriorShape = "laplace";
      eloUnratedCenter = 700;
      eloUnratedK = 260;
      eloLoosenessUnratedPercent = 300;
    } else {
      eloPriorShape = "flat";
    }
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

  function setHandicapWielRule(v: boolean) {
    handicapWielRule = v;
    persist();
  }

  function setHalfPointAbsences(v: boolean) {
    halfPointAbsences = v;
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

  <fieldset class="swiss-fieldset" disabled={swissDisabled}>
  <div class="section">
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

  <div class="subsection">
    <h4>{$_("settings.degressiveTitle")}</h4>
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

  <div class="subsection">
    <h4>{$_("settings.airtightGroupsTitle")}</h4>
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

  <div class="subsection">
    <h4>{$_("settings.macmahonFromEloTitle")}</h4>
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
  </div>
  </div>

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

  <fieldset class="floater-fieldset" disabled={floaterDisabled}>
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
    <label class="check">
      <input
        type="checkbox"
        checked={handicapWielRule}
        disabled={busy}
        onchange={(e) => setHandicapWielRule(e.currentTarget.checked)}
      />
      {$_("settings.handicapWielCheckbox")}
    </label>
    <p class="desc">
      {$_("settings.handicapWielDesc")}
    </p>
    <label class="check">
      <input
        type="checkbox"
        checked={halfPointAbsences}
        disabled={busy}
        onchange={(e) => setHalfPointAbsences(e.currentTarget.checked)}
      />
      {$_("settings.halfPointAbsencesCheckbox")}
    </label>
    <p class="desc">
      {$_("settings.halfPointAbsencesDesc")}
    </p>
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

  <div class="section">
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
    {#if eloEstimateLive}
      <p class="desc small-note">
        {$_("settings.eloEstimateKnobsNote")}
      </p>
      <label class="check elo-k">
        {$_("settings.eloApplyTo")}
        <select
          class="tb-select"
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
          class="tb-select"
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
    {/if}
  </div>

  <div class="section">
    <h3>{$_("settings.exportSettings")}</h3>
    <p class="desc">{$_("settings.exportSettingsDesc")}</p>
    <button type="button" class="ghost small" onclick={exportSettings}>
      {$_("settings.exportSettings")}
    </button>
  </div>
</div>

<style>
  .settings {
    max-width: 32rem;
  }
  @media (min-width: 60rem) {
    .settings {
      max-width: 66rem;
      margin: 0 auto;
      column-count: 2;
      column-gap: 3rem;
    }
    .section,
    fieldset.swiss-fieldset,
    fieldset.floater-fieldset {
      break-inside: avoid;
    }
  }
  h3 {
    margin: 0.4rem 0 0.3rem;
  }
  .section {
    margin-top: 1.75rem;
    border-top: 1px solid var(--border-divider);
    padding-top: 1rem;
  }
  .settings > fieldset.swiss-fieldset > .section:first-child {
    margin-top: 0.5rem;
    border-top: none;
    padding-top: 0;
  }
  /* Groups the Swiss-only sections so they can be greyed out as one in ELO mode.
     Reset the browser's default fieldset chrome; the inner `.section`s keep their
     own separators. It's the first thing in the tab, so no leading divider. */
  fieldset.swiss-fieldset {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  fieldset.swiss-fieldset:disabled {
    opacity: 0.5;
  }
  /* Floater selection is greyed out in pure ELO mode (replaced by the ELO-gap
     rule), sharing the disabled styling with the rest of the swiss-fieldset. */
  fieldset.floater-fieldset {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  fieldset.floater-fieldset:disabled {
    opacity: 0.5;
  }
  .subsection {
    margin-top: 1.25rem;
  }
  .subsection h4 {
    margin: 0 0 0.3rem;
    font-size: 0.95rem;
    color: var(--text-strong);
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
