<script lang="ts">
  import { untrack } from "svelte";
  import { _ } from "svelte-i18n";
  import type {
    ClubProtection,
    CupFormat,
    EloEstimator,
    EloPriorShape,
    HandicapPolicy,
    PairingMode,
    NationalityProtection,
    Player,
    PlayerCategory,
    Tiebreak,
    TournamentSettings,
  } from "../types";
  import { loadSettings, saveSettings } from "../tournamentFile";
  import { cleanThresholds, eqThresholds, normExempt, type ThresholdRow } from "../thresholds";
  import { handicapChoice, type HandicapChoice } from "../handicap";
  import EventSection from "./settings/EventSection.svelte";
  import MacMahonSection from "./settings/MacMahonSection.svelte";
  import OtherRulesSection from "./settings/OtherRulesSection.svelte";
  import PairingModeSection from "./settings/PairingModeSection.svelte";
  import PairingRulesSection from "./settings/PairingRulesSection.svelte";
  import RankingSection from "./settings/RankingSection.svelte";
  import TournamentModeSection from "./settings/TournamentModeSection.svelte";
  import type { TournamentMode } from "./settings/modes";

  interface Props {
    settings: TournamentSettings;
    /** The tournament's name, shown in the American Grid header preview. */
    tournamentName: string;
    /** Registration already finalized — edits here get a warning. */
    finalized: boolean;
    /** The registered players, used to suggest club and nationality names for
     *  exemptions. */
    players: Player[];
    onUpdate: (settings: TournamentSettings) => void;
    busy?: boolean;
  }

  let {
    settings,
    tournamentName,
    finalized,
    players,
    onUpdate,
    busy: appBusy = false,
  }: Props = $props();

  // Every control on this page saves on `change`, and every control is disabled
  // while that save is in flight. Greying out the instant the save starts breaks
  // Tab: leaving the City field commits it, which disables the whole form —
  // including the Country field the browser was about to move focus to — so the
  // focus lands nowhere and the next keystrokes go to the page instead of the
  // field. A settings save is a few milliseconds against the local server, so
  // wait a beat before greying anything out: a normal save now never disables a
  // control, and Tab walks the form as it should. A save slow enough to be worth
  // showing still locks the form, just late; an edit that manages to race into
  // the grace window is caught by the server's version check and comes back as a
  // loud 409, not a silent overwrite.
  const GREY_OUT_AFTER_MS = 250;
  let busy = $state(false);
  $effect(() => {
    if (!appBusy) {
      busy = false;
      return;
    }
    const timer = setTimeout(() => (busy = true), GREY_OUT_AFTER_MS);
    return () => clearTimeout(timer);
  });

  // Team mode and team size are structural: they shape every roster and every
  // board, so the server freezes them at finalization. Disabling the controls
  // there beats letting the referee submit a change the server will reject.
  const locked = $derived(finalized);

  // Download the current settings as JSON (for the simulation CLI's --configs,
  // or to share a configuration). Fire-and-forget: a cancelled dialog is a no-op.
  function exportSettings() {
    void saveSettings("tournament", settings);
  }

  // Load a settings JSON file and apply it. The server validates the result (and
  // any error surfaces on the app's banner via onUpdate → run); only the file
  // read / parse can fail here, which we show inline. A cancelled dialog is a
  // no-op. The synced-from-prop $effect refreshes every control afterwards.
  let importError = $state<string | null>(null);
  async function importSettings() {
    importError = null;
    try {
      const loaded = await loadSettings();
      if (loaded) onUpdate(loaded);
    } catch (err) {
      importError = err instanceof Error ? err.message : String(err);
    }
  }

  function eqStr(a: string[], b: string[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
  }

  // Canonical form of the category rows — trimmed names, blanks dropped, first of
  // any repeated id kept — mirroring the server's normalization. Used both in the
  // "did our edit round-trip?" check and when persisting, so a half-typed blank
  // row never counts as a divergence from the stored settings.
  function cleanCategories(rows: PlayerCategory[]): PlayerCategory[] {
    const seen = new Set<string>();
    const out: PlayerCategory[] = [];
    for (const c of rows) {
      const name = c.name.trim();
      if (name.length === 0 || seen.has(c.id)) continue;
      seen.add(c.id);
      out.push({ id: c.id, name });
    }
    return out;
  }

  function eqCategories(a: PlayerCategory[], b: PlayerCategory[]): boolean {
    return a.length === b.length && a.every((c, i) => c.id === b[i].id && c.name === b[i].name);
  }

  // The event's identity, for the American Grid header (empty = not entered).
  // The date inputs are `type="date"`, so their value is already the ISO
  // `YYYY-MM-DD` the server accepts — and comparing two of them as strings is
  // comparing them chronologically.
  let city = $state("");
  let country = $state("");
  let firstDate = $state("");
  let lastDate = $state("");
  let timeControl = $state("");

  // Local editable rows, kept in *entry* order (not sorted) so the row a referee
  // is editing never jumps or shows a stale value. The inputs bind to these.
  let thresholds = $state<ThresholdRow[]>([]);
  let airtightRounds = $state<number | null>(null);
  let clubEnabled = $state(false);
  let clubRounds = $state<number | null>(null);
  let exemptClubs = $state<string[]>([]);
  // Nationality protection: the same three controls, one rule tier weaker.
  let nationalityEnabled = $state(false);
  let nationalityRounds = $state<number | null>(null);
  let exemptNationalities = $state<string[]>([]);
  let floaterStyle = $state<"classic" | "median">("classic");
  let cupEnabled = $state(false);
  let cupFormat = $state<CupFormat>("direct");
  let longBoardsEnabled = $state(false);
  // Team mode: whether teams are the unit of pairing, and how many players each
  // has. `teamSize` keeps its last value while the mode is off, so unticking and
  // re-ticking doesn't lose the referee's choice.
  let teamMode = $state(false);
  let teamSize = $state(3);
  let handicapPolicy = $state<HandicapChoice>("allowed");
  let handicapWielRule = $state(false);
  let halfPointAbsences = $state(false);
  let tiebreaks = $state<Tiebreak[]>([]);
  // Local editable category rows (id + name), in entry order — the same
  // entry-order-preserving pattern as the thresholds above.
  let categories = $state<PlayerCategory[]>([]);
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

  // The three mutually exclusive formats, as one radio group (see
  // `settings/modes.ts` for why each excludes the others).
  const tournamentMode = $derived<TournamentMode>(
    teamMode ? "team" : cupEnabled ? "cup" : "normal",
  );

  // Whether the Swiss knobs (MacMahon, floater, club/nationality protection)
  // apply at all — everywhere except the pure ELO mode, which replaces them.
  // They are removed rather than greyed out there: `persist` doesn't send them in
  // ELO mode either, so showing them would advertise settings nothing stores.
  const swissActive = $derived(!eloEnabled);

  // Whether to offer estimate-based MacMahon: it compares the estimate against
  // ELO thresholds, so it needs one to do anything. Kept on screen when already
  // on but inert (no ELO threshold left), rather than hiding a flag that is set.
  const showMacmahonFromElo = $derived(hasEloThreshold || macmahonFromElo);

  // Whether the estimated-ELO tie-break is a meaningful *ranking* criterion: only
  // in the ELO pairing mode, where the estimate is what the tournament runs on
  // (under estimate-based MacMahon it is an input to the MacMahon points, shown in
  // their tooltip on the Results tab, not a column of its own), and only while
  // rated players are actually estimated (k = 0 pins them to their registration
  // rating, so ranking by it would just be ranking by that rating). Mirrors the
  // server's `est_elo_ranks` normalization gate.
  const estEloRanks = $derived(eloEnabled && eloKPercent !== 0);

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

  // Adopt the persisted settings only on a genuine external change — a load, an
  // undo, or the server normalizing our input. When our own edit merely
  // round-trips (the server just sorts/dedups it into the same canonical form),
  // our local state already matches, so we keep the entry order rather than
  // reshuffling under the cursor. `untrack` makes this fire on `settings` changes
  // only, not our own writes.
  $effect(() => {
    // The Swiss knobs live under pairing.swiss; the ELO estimator lives under
    // pairing.elo.estimator OR pairing.swiss.macmahon.source (estimate-MacMahon).
    const pairing = settings.pairing;
    const swiss = pairing.kind === "swiss" ? pairing : null;
    const macmahonSource = swiss?.macmahon.source;
    const est =
      pairing.kind === "elo"
        ? pairing.estimator
        : macmahonSource?.kind === "from_estimate"
          ? macmahonSource.estimator
          : null;
    const sCity = settings.city ?? "";
    const sCountry = settings.country ?? "";
    const sFirstDate = settings.dates?.first ?? "";
    const sLastDate = settings.dates?.last ?? "";
    const sTimeControl = settings.time_control ?? "";
    const sThresholds = swiss?.macmahon.thresholds ?? [];
    const sAirtight = swiss?.airtight_groups ?? null;
    const cp = swiss?.club_protection;
    const sEnabled = cp?.kind === "on";
    const sRounds = cp?.kind === "on" ? (cp.rounds ?? null) : null;
    const sExempt = cp?.kind === "on" ? (cp.exempt_clubs ?? []) : [];
    const np = swiss?.nationality_protection;
    const sNatEnabled = np?.kind === "on";
    const sNatRounds = np?.kind === "on" ? (np.rounds ?? null) : null;
    const sNatExempt = np?.kind === "on" ? (np.exempt_nationalities ?? []) : [];
    const sFloater = swiss?.floater_style ?? "classic";
    const sCup = settings.cup_enabled;
    const sCupFormat = settings.cup_format ?? "direct";
    const sLong = settings.long_boards_enabled ?? false;
    const sTeamMode = settings.teams != null;
    const sTeamSize = settings.teams?.size ?? 3;
    const sHandicap = handicapChoice(settings.handicap_policy);
    const sHandicapWiel =
      settings.handicap_policy.kind === "enabled" ? (settings.handicap_policy.wiel_rule ?? false) : false;
    const sHalfPointAbsences = settings.half_point_absences ?? false;
    const sTiebreaks = settings.tiebreaks ?? [];
    const sCategories = settings.categories ?? [];
    const sElo = pairing.kind === "elo";
    const sMacmahonFromElo = macmahonSource?.kind === "from_estimate";
    const sEloK = est?.k_multiplier ?? 100;
    const sEloProv = est?.provisional_multiplier ?? 200;
    const sEloUnratedCenter = est?.unrated_prior_center ?? 600;
    const sEloUnratedK = est?.unrated_k ?? 705;
    const sEloPriorShape = est?.prior_shape_unrated ?? "gaussian";
    const sEloLooseEst = est?.upward_looseness_established ?? 100;
    const sEloLooseProv = est?.upward_looseness_provisional ?? 100;
    const sEloLooseUnr = est?.upward_looseness_unrated ?? 100;
    untrack(() => {
      const matches =
        city.trim() === sCity &&
        country.trim() === sCountry &&
        firstDate === sFirstDate &&
        lastDate === sLastDate &&
        timeControl.trim() === sTimeControl &&
        eqThresholds(cleanThresholds(thresholds), sThresholds) &&
        (airtightRounds ?? null) === sAirtight &&
        clubEnabled === sEnabled &&
        (clubRounds ?? null) === sRounds &&
        eqStr(normExempt(exemptClubs), sExempt) &&
        nationalityEnabled === sNatEnabled &&
        (nationalityRounds ?? null) === sNatRounds &&
        eqStr(normExempt(exemptNationalities), sNatExempt) &&
        floaterStyle === sFloater &&
        cupEnabled === sCup &&
        cupFormat === sCupFormat &&
        longBoardsEnabled === sLong &&
        teamMode === sTeamMode &&
        teamSize === sTeamSize &&
        handicapPolicy === sHandicap &&
        handicapWielRule === sHandicapWiel &&
        halfPointAbsences === sHalfPointAbsences &&
        eqStr(tiebreaks, sTiebreaks) &&
        eqCategories(cleanCategories(categories), sCategories) &&
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
        city = sCity;
        country = sCountry;
        firstDate = sFirstDate;
        lastDate = sLastDate;
        timeControl = sTimeControl;
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
        nationalityEnabled = sNatEnabled;
        nationalityRounds = sNatRounds;
        exemptNationalities = [...sNatExempt];
        floaterStyle = sFloater;
        cupEnabled = sCup;
        cupFormat = sCupFormat;
        longBoardsEnabled = sLong;
        teamMode = sTeamMode;
        teamSize = sTeamSize;
        handicapPolicy = sHandicap;
        handicapWielRule = sHandicapWiel;
        halfPointAbsences = sHalfPointAbsences;
        tiebreaks = [...sTiebreaks];
        categories = sCategories.map((c) => ({ id: c.id, name: c.name }));
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
    // The estimator config the current controls describe — attached to whichever
    // mode carries a live estimate (ELO pairing, or estimate-based MacMahon).
    const estimator: EloEstimator = {
      k_multiplier: eloKPercent,
      provisional_multiplier: eloProvisionalPercent,
      unrated_prior_center: eloUnratedCenter,
      unrated_k: eloUnratedK,
      prior_shape_established: "gaussian",
      prior_shape_provisional: "gaussian",
      prior_shape_unrated: eloPriorShape,
      upward_looseness_established: eloLoosenessEstablishedPercent,
      upward_looseness_provisional: eloLoosenessProvisionalPercent,
      upward_looseness_unrated: eloLoosenessUnratedPercent,
    };
    const pairing: PairingMode = eloEnabled
      ? { kind: "elo", estimator }
      : {
          kind: "swiss",
          floater_style: floaterStyle,
          airtight_groups: airtightRounds,
          club_protection: clubEnabled
            ? ({
                kind: "on",
                rounds: clubRounds,
                exempt_clubs: exemptClubs.map((c) => c.trim()).filter((c) => c.length > 0),
              } satisfies ClubProtection)
            : ({ kind: "off" } satisfies ClubProtection),
          nationality_protection: nationalityEnabled
            ? ({
                kind: "on",
                rounds: nationalityRounds,
                exempt_nationalities: exemptNationalities
                  .map((c) => c.trim())
                  .filter((c) => c.length > 0),
              } satisfies NationalityProtection)
            : ({ kind: "off" } satisfies NationalityProtection),
          macmahon: {
            thresholds: cleanThresholds(thresholds),
            source: macmahonFromElo ? { kind: "from_estimate", estimator } : { kind: "static" },
          },
        };
    onUpdate({
      city: city.trim() || null,
      country: country.trim() || null,
      // The dates travel as a pair or not at all — the server rejects a
      // half-filled range, and the edit handlers below keep the two inputs
      // filled (and ordered) together so that can't be reached from here.
      dates: firstDate && lastDate ? { first: firstDate, last: lastDate } : null,
      time_control: timeControl.trim() || null,
      pairing,
      cup_enabled: cupEnabled,
      cup_format: cupFormat,
      long_boards_enabled: longBoardsEnabled,
      teams: teamMode ? { size: teamSize } : undefined,
      handicap_policy:
        handicapPolicy === "none"
          ? ({ kind: "none" } satisfies HandicapPolicy)
          : ({
              kind: "enabled",
              display: handicapPolicy,
              wiel_rule: handicapWielRule,
            } satisfies HandicapPolicy),
      half_point_absences: halfPointAbsences,
      tiebreaks: [...tiebreaks],
      categories: cleanCategories(categories),
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

  /** Drop the estimated-ELO tie-break once it stops being a valid ranking
   * criterion — leaving the ELO pairing mode, or pinning the rated players.
   * Mirrors the server's normalization, so our local order matches what comes
   * back. */
  function dropEstEloUnlessRanking() {
    if (!estEloRanks) tiebreaks = tiebreaks.filter((c) => c !== "est_elo");
  }

  /** Board wins is a team criterion; leaving team mode takes it with it. The
   *  server does this too (`TournamentSettings::normalized`) and is the
   *  authority — this only spares the referee seeing it linger for a round
   *  trip. */
  function dropBoardWinsUnlessTeams() {
    if (!teamMode) tiebreaks = tiebreaks.filter((c) => c !== "board_wins");
  }

  function setPairingMode(mode: "swiss" | "elo") {
    eloEnabled = mode === "elo";
    if (eloEnabled) applyEstimatorDefaultsIfUnset();
    dropEstEloUnlessRanking();
    persist();
  }

  function setMacmahonFromElo(on: boolean) {
    macmahonFromElo = on;
    if (on) applyEstimatorDefaultsIfUnset();
    persist();
  }

  // Control 1 — who the estimate applies to. k = 0 pins rated players to their
  // registration rating (estimate unrated only); k = 100% estimates everyone.
  function setEloApplyTo(v: "unrated" | "all") {
    eloKPercent = v === "unrated" ? 0 : 100;
    // Pinning the rated players also retires the estimate as a ranking criterion.
    dropEstEloUnlessRanking();
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

  // Turning team mode on adopts the team tie-break order — match points, then
  // board wins, then the SOS family, which is what team events use — but only
  // while the order is still the untouched individual default. A referee who has
  // reordered the criteria keeps their order.
  const INDIVIDUAL_DEFAULT_TIEBREAKS: Tiebreak[] = ["points", "sos_m", "sodos_m", "sosos_m"];
  const TEAM_DEFAULT_TIEBREAKS: Tiebreak[] = [
    "points",
    "board_wins",
    "sos_m",
    "sodos_m",
    "sosos_m",
  ];

  function setTournamentMode(mode: TournamentMode) {
    const wasTeam = teamMode;
    teamMode = mode === "team";
    cupEnabled = mode === "cup";
    // Both other formats are Swiss-based, and the pairing-mode radio is only
    // offered in the normal mode — so leaving ELO set would strand the
    // tournament in a mode with no control to leave it by.
    if (mode !== "normal") eloEnabled = false;
    if (teamMode && !wasTeam && eqStr(tiebreaks, INDIVIDUAL_DEFAULT_TIEBREAKS)) {
      tiebreaks = [...TEAM_DEFAULT_TIEBREAKS];
    }
    // The estimated-ELO tie-break ranks nothing outside the ELO pairing mode,
    // and board wins nothing outside team mode.
    dropEstEloUnlessRanking();
    dropBoardWinsUnlessTeams();
    // The rest of what team mode forbids (long games, grade thresholds) is left
    // to the server, which rejects the pair naming the conflict rather than
    // silently discarding whichever side the referee did not just touch.
    persist();
  }
</script>

<!-- One column of sections, most-used first. A section that cannot apply to the
     current format is removed rather than greyed out; only what finalization
     *locks* stays on screen disabled, since the referee still needs to read it.

     Each section is its own component: the controls of one setting group, and
     the handlers that edit that group's slice of the state above. They all save
     through this component's `persist`, so there is still exactly one place a
     settings change leaves from. -->
<div class="settings">
  {#if finalized}
    <p class="hint warning">
      ⚠ {$_("settings.finalizedWarning")}
    </p>
  {/if}

  <EventSection
    bind:city
    bind:country
    bind:firstDate
    bind:lastDate
    bind:timeControl
    {tournamentName}
    {busy}
    {persist}
  />

  <TournamentModeSection
    {tournamentMode}
    {teamMode}
    bind:teamSize
    bind:cupFormat
    {locked}
    {busy}
    {setTournamentMode}
    {persist}
  />

  <!-- The pure ELO mode is incompatible with a team tournament (the server
       rejects the pair) and pulls against a cup, so the choice is only offered
       for the normal format; the other two are Swiss by construction. -->
  {#if tournamentMode === "normal"}
    <PairingModeSection
      {pairingMode}
      {eloEnabled}
      {eloApplyTo}
      {unratedPrior}
      {busy}
      {setPairingMode}
      {setEloApplyTo}
      {setUnratedPrior}
    />
  {/if}

  {#if swissActive}
    <MacMahonSection
      bind:thresholds
      bind:airtightRounds
      {teamMode}
      {players}
      {macmahonFromElo}
      {hasEloThreshold}
      {showMacmahonFromElo}
      {eloApplyTo}
      {unratedPrior}
      {busy}
      {setMacmahonFromElo}
      {setEloApplyTo}
      {setUnratedPrior}
      {persist}
    />

    <PairingRulesSection
      bind:floaterStyle
      bind:clubEnabled
      bind:clubRounds
      bind:exemptClubs
      bind:nationalityEnabled
      bind:nationalityRounds
      bind:exemptNationalities
      {players}
      {busy}
      {persist}
    />
  {/if}

  <RankingSection bind:tiebreaks {estEloRanks} {teamMode} {busy} {persist} />

  <OtherRulesSection
    bind:handicapPolicy
    bind:handicapWielRule
    bind:halfPointAbsences
    bind:categories
    bind:longBoardsEnabled
    {teamMode}
    {busy}
    {persist}
  />

  <div class="io-footer">
    <div class="settings-io">
      <button type="button" class="ghost small" onclick={exportSettings}>
        {$_("settings.exportSettings")}
      </button>
      <button type="button" class="ghost small" onclick={importSettings} disabled={busy}>
        {$_("settings.importSettings")}
      </button>
    </div>
    <p class="desc small-note">{$_("settings.exportSettingsDesc")}</p>
    {#if importError}
      <p class="import-error" role="alert">{importError}</p>
    {/if}
  </div>
</div>

<style>
  /* The page's own chrome. Everything below it is `:global`, because the markup
     it styles is rendered by the section components — the `.settings` wrapper
     still carries this component's scope, so the rules reach that markup and
     nothing outside this page. Section-specific rules live with their
     section. */
  .settings {
    max-width: 76rem;
    margin: 0 auto;
  }
  .hint.warning {
    color: var(--color-warning);
    font-size: 0.85rem;
    margin: 0 0 1rem;
  }
  .io-footer {
    margin-top: 1.75rem;
    border-top: 1px solid var(--border-divider);
    padding-top: 1rem;
  }
  .settings-io {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .import-error {
    color: var(--color-danger);
    font-size: 0.85rem;
    margin: 0.5rem 0 0;
  }
  .settings :global(h3) {
    margin: 0.4rem 0 0.3rem;
  }
  .settings :global(.section) {
    margin-top: 1.75rem;
    border-top: 1px solid var(--border-divider);
    padding-top: 1rem;
  }
  /* The event section opens the tab, so it has nothing above to be separated
     from. (`:first-of-type` among the `section` children — the finalized warning
     is a `p`.) */
  .settings > :global(section.section:first-of-type) {
    margin-top: 0.5rem;
    border-top: none;
    padding-top: 0;
  }
  /* Lays a section's subsections out side by side when there is room to: with
     `auto-fit`, the browser fits as many tracks of at least `--col-min` as the
     width allows and stretches them to fill it, collapsing to a single column on
     a narrow window — so the number of columns follows the window with no
     breakpoint to maintain. `min(100%, …)` keeps a track from overflowing a
     container narrower than `--col-min` itself. Each section sets its own
     `--col-min`: enough for its widest control, and no more. */
  .settings :global(.grid) {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, var(--col-min, 20rem)), 1fr));
    gap: 1.25rem 2.5rem;
    align-items: start;
  }
  /* Subsections. `fieldset`/`legend` rather than a `div` and a heading: these are
     genuine control groups (several are radio groups), and it is what names them
     to a screen reader. The browser's own chrome is reset; `min-width: 0`
     overrides the `min-content` floor a fieldset would otherwise impose as a grid
     item, which would stop the tracks from shrinking. */
  .settings :global(fieldset.sub) {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  .settings :global(fieldset.sub > legend) {
    padding: 0;
    margin: 0 0 0.3rem;
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--text-strong);
  }
  .settings :global(.desc) {
    color: var(--text-secondary);
    font-size: 0.85rem;
    margin: 0 0 1rem;
    line-height: 1.4;
  }
  .settings :global(.small-note) {
    margin: 0.5rem 0 0;
  }
  .settings :global(.check) {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.9rem;
    color: var(--text-strong);
  }
  .settings :global(.check input[type="checkbox"]) {
    width: 1rem;
    height: 1rem;
  }
  .settings :global(.check + .check) {
    margin-top: 0.4rem;
  }
  .settings :global(.muted) {
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  .settings :global(.thresholds) {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-items: flex-start;
  }
  /* Wraps rather than overflowing: a threshold row is the widest control here and
     a narrow window (or a narrow track) has less than its full width. */
  .settings :global(.threshold-row) {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .settings :global(.threshold) {
    width: 6rem;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .settings :global(.threshold.narrow) {
    width: 4rem;
  }
  .settings :global(.threshold-kind) {
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .settings :global(.tb-select) {
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .settings :global(.remove) {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-secondary);
    cursor: pointer;
    border-radius: 0.4rem;
    padding: 0.1rem 0.4rem;
  }
  .settings :global(.remove:hover:not(:disabled)) {
    color: var(--color-danger);
    border-color: var(--border-soft);
  }
  .settings :global(.ghost) {
    background: transparent;
    border: 1px solid var(--border-soft);
    color: inherit;
    border-radius: 0.4rem;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
    font: inherit;
  }
  .settings :global(.ghost.small) {
    font-size: 0.85rem;
  }
  .settings :global(.ghost:hover:not(:disabled)) {
    background: var(--bg-hover);
  }
  .settings :global(.ghost:disabled) {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
