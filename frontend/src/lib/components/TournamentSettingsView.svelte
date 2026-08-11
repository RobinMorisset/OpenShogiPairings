<script lang="ts">
  import { untrack } from "svelte";
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type {
    ClubProtection,
    CupFormat,
    EloEstimator,
    EloPriorShape,
    GradeKind,
    HandicapPolicy,
    PairingMode,
    MacMahonThreshold,
    NationalityProtection,
    Player,
    PlayerCategory,
    Tiebreak,
    ThresholdCriterion,
    TournamentSettings,
  } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { loadSettings, saveSettings } from "../tournamentFile";
  import { gradeRank } from "../grade";
  import { cleanThresholds, eqThresholds, normExempt, type ThresholdRow } from "../thresholds";
  import { handicapChoice, type HandicapChoice } from "../handicap";

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
    busy = false,
  }: Props = $props();

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

  // The three mutually exclusive formats, as one radio group. Team mode is
  // exclusive by the server's own rule (`team_mode_conflict` rejects the cup and
  // the ELO pairing alongside it); the hybrid cup is exclusive here by choice — a
  // bracket exists to find a single winner, which is the opposite of what the
  // pure ELO mode optimizes for, so the cup implies the Swiss pairing.
  type TournamentMode = "normal" | "team" | "cup";
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

  // Whether a live ELO estimate is maintained at all: the ELO pairing mode, or
  // estimate-based MacMahon with an ELO threshold to compare against. Mirrors the
  // server's `elo_estimate_live`.
  const eloEstimateLive = $derived(
    eloEnabled || (macmahonFromElo && hasEloThreshold),
  );

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

  // Metrics not yet in the ranking order — the choices for the "add" dropdown.
  // Estimated ELO is only meaningful as a ranking criterion when the estimate is
  // maintained *and* applies to rated players (otherwise it just sits at the
  // registration rating), so it isn't offered otherwise.
  const availableTiebreaks = $derived(
    TIEBREAKS.filter(
      (t) => !tiebreaks.includes(t.code) && (estEloRanks || t.code !== "est_elo"),
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

  function editCity(raw: string) {
    city = raw;
    persist();
  }

  function editCountry(raw: string) {
    country = raw;
    persist();
  }

  // The two dates are one setting: the header needs both the range and its
  // closing date, so entering one fills the other in (visibly, in the input) —
  // the one-day case — and clearing one clears the pair. A last day earlier than
  // the first is likewise pulled back into order rather than sent for the server
  // to reject.
  function editFirstDate(raw: string) {
    firstDate = raw;
    if (!raw) lastDate = "";
    else if (!lastDate || lastDate < raw) lastDate = raw;
    persist();
  }

  function editLastDate(raw: string) {
    lastDate = raw;
    if (!raw) firstDate = "";
    else if (!firstDate || firstDate > raw) firstDate = raw;
    persist();
  }

  function editTimeControl(raw: string) {
    timeControl = raw;
    persist();
  }

  function setFloaterStyle(v: "classic" | "median") {
    floaterStyle = v;
    persist();
  }

  function setCupFormat(v: CupFormat) {
    cupFormat = v;
    persist();
  }

  function setLongBoardsEnabled(v: boolean) {
    longBoardsEnabled = v;
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
    // The estimated-ELO tie-break ranks nothing outside the ELO pairing mode.
    dropEstEloUnlessRanking();
    // The rest of what team mode forbids (long games, grade thresholds) is left
    // to the server, which rejects the pair naming the conflict rather than
    // silently discarding whichever side the referee did not just touch.
    persist();
  }

  function setTeamSize(v: number) {
    teamSize = v;
    persist();
  }

  function setHandicapPolicy(v: HandicapChoice) {
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

  function addCategory() {
    // A fresh client-minted id; the row persists once it gets a non-blank name
    // (a blank one is dropped by `cleanCategories`, so no request is sent yet).
    categories.push({ id: crypto.randomUUID(), name: "" });
  }

  function removeCategory(i: number) {
    categories.splice(i, 1);
    persist();
  }

  function editCategoryName(i: number, raw: string) {
    categories[i].name = raw;
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

  // The header the American Grid export will carry, shown live under the fields
  // so the referee sees what FESA receives. Mirrors `header_lines` in
  // `american_grid.rs`; keep the two in step.
  const headerPreview = $derived.by(() => {
    const range = !firstDate || !lastDate
      ? null
      : firstDate === lastDate
        ? firstDate
        : `${firstDate} to ${lastDate}`;
    const parts = [tournamentName, city.trim(), country.trim(), range].filter(Boolean);
    const lines = [`[${parts.join(", ")}]`];
    if (range) lines.push(`[${lastDate}]`);
    const tc = timeControl.trim();
    if (tc) lines.push(`[Time control: ${tc}]`);
    return lines.join("\n");
  });

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
<!-- The two controls of the live ELO estimate. Rendered next to whichever switch
     turns the estimate on — the pure ELO pairing mode, or estimate-based
     MacMahon — rather than in one fixed place, so the knobs are never in a
     different section from the option that gives them an effect. -->
{#snippet estimatorKnobs()}
  <div class="estimator">
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
  </div>
{/snippet}

<!-- One column of sections, most-used first. A section that cannot apply to the
     current format is removed rather than greyed out; only what finalization
     *locks* stays on screen disabled, since the referee still needs to read it. -->
<div class="settings">
  {#if finalized}
    <p class="hint warning">
      ⚠ {$_("settings.finalizedWarning")}
    </p>
  {/if}

  <section class="section">
    <h3>{$_("settings.eventTitle")}</h3>
    <p class="desc">{$_("settings.eventDesc")}</p>
    <div class="grid event-fields">
      <label class="field">
        <span>{$_("settings.eventCity")}</span>
        <input
          type="text"
          value={city}
          disabled={busy}
          onchange={(e) => editCity(e.currentTarget.value)}
        />
      </label>
      <label class="field">
        <span>{$_("settings.eventCountry")}</span>
        <input
          type="text"
          value={country}
          disabled={busy}
          onchange={(e) => editCountry(e.currentTarget.value)}
        />
      </label>
      <label class="field">
        <span>{$_("settings.eventFirstDay")}</span>
        <input
          type="date"
          value={firstDate}
          disabled={busy}
          onchange={(e) => editFirstDate(e.currentTarget.value)}
        />
      </label>
      <label class="field">
        <span>{$_("settings.eventLastDay")}</span>
        <input
          type="date"
          value={lastDate}
          disabled={busy}
          onchange={(e) => editLastDate(e.currentTarget.value)}
        />
      </label>
      <label class="field">
        <span>{$_("settings.eventTimeControl")}</span>
        <input
          type="text"
          value={timeControl}
          placeholder={$_("settings.eventTimeControlPlaceholder")}
          disabled={busy}
          onchange={(e) => editTimeControl(e.currentTarget.value)}
        />
      </label>
    </div>
    <!-- Collapsed: it is a preview of an export detail, not something to keep
         an eye on while filling the fields in. -->
    <details class="preview-details">
      <summary>{$_("settings.eventHeaderPreview")}</summary>
      <pre class="header-preview">{headerPreview}</pre>
    </details>
  </section>

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

  <!-- The pure ELO mode is incompatible with a team tournament (the server
       rejects the pair) and pulls against a cup, so the choice is only offered
       for the normal format; the other two are Swiss by construction. -->
  {#if tournamentMode === "normal"}
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
        {@render estimatorKnobs()}
      {/if}
    </section>
  {/if}

  {#if swissActive}
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
                  class="threshold-kind"
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
              {@render estimatorKnobs()}
            {/if}
          </fieldset>
        {/if}

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
              class="threshold narrow"
              value={airtightRounds ?? 1}
              disabled={busy || airtightRounds == null}
              onchange={(e) => editAirtightRounds(e.currentTarget.value)}
            />
            {$_("settings.onlyFirstRoundsSuffix")}
          </label>
        </fieldset>
      </div>
    </section>

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
                  class="threshold narrow"
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
                      class="club-input"
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
                  class="ghost small"
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
  {/if}

  <section class="section">
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
        <div class="threshold-row tb-add">
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
  </section>

  <section class="section">
    <h3>{$_("settings.otherRulesTitle")}</h3>
    <div class="grid other-grid">
      <fieldset class="sub">
        <legend>{$_("settings.handicapTitle")}</legend>
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
        <p class="desc small-note">
          {$_("settings.handicapWielDesc")}
        </p>
      </fieldset>

      <fieldset class="sub">
        <legend>{$_("settings.absencesTitle")}</legend>
        <p class="desc">
          {$_("settings.absencesDesc")}
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
        <p class="desc small-note">
          {$_("settings.halfPointAbsencesDesc")}
        </p>
      </fieldset>

      <fieldset class="sub">
        <legend>{$_("settings.categoriesTitle")}</legend>
        <p class="desc">
          {$_("settings.categoriesDesc")}
        </p>
        <div class="categories">
          {#each categories as row, i (row.id)}
            <div class="category-row">
              <input
                type="text"
                class="category-name"
                value={row.name}
                placeholder={$_("settings.categoryNamePlaceholder")}
                disabled={busy}
                onchange={(e) => editCategoryName(i, e.currentTarget.value)}
              />
              <button
                type="button"
                class="remove"
                disabled={busy}
                title={$_("settings.removeCategory")}
                onclick={() => removeCategory(i)}>✕</button
              >
            </div>
          {/each}
          {#if categories.length === 0}
            <p class="muted">{$_("settings.noCategories")}</p>
          {/if}
          <button
            type="button"
            class="ghost small"
            disabled={busy}
            onclick={addCategory}>{$_("settings.addCategory")}</button
          >
        </div>
      </fieldset>

      <!-- A team match is one board per player; a board spanning two rounds has
           no reading there, and the server rejects the pair. -->
      {#if !teamMode}
        <fieldset class="sub">
          <legend>{$_("settings.longBoardsTitle")}</legend>
          <p class="desc">
            {$_("settings.longBoardsDesc")}
          </p>
          <label class="check">
            <input
              type="checkbox"
              checked={longBoardsEnabled}
              disabled={busy}
              onchange={(e) => setLongBoardsEnabled(e.currentTarget.checked)}
            />
            {$_("settings.longBoardsCheckbox")}
          </label>
        </fieldset>
      {/if}
    </div>
  </section>

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
  .settings {
    max-width: 76rem;
    margin: 0 auto;
  }
  h3 {
    margin: 0.4rem 0 0.3rem;
  }
  .section {
    margin-top: 1.75rem;
    border-top: 1px solid var(--border-divider);
    padding-top: 1rem;
  }
  /* The event section opens the tab, so it has nothing above to be separated
     from. (`:first-of-type` among the `section` children — the finalized warning
     is a `p`.) */
  .settings > section.section:first-of-type {
    margin-top: 0.5rem;
    border-top: none;
    padding-top: 0;
  }
  /* Lays a section's subsections out side by side when there is room to: with
     `auto-fit`, the browser fits as many tracks of at least `--col-min` as the
     width allows and stretches them to fill it, collapsing to a single column on
     a narrow window — so the number of columns follows the window with no
     breakpoint to maintain. `min(100%, …)` keeps a track from overflowing a
     container narrower than `--col-min` itself. */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, var(--col-min, 20rem)), 1fr));
    gap: 1.25rem 2.5rem;
    align-items: start;
  }
  /* Per-section track width: enough for that section's widest control, and no
     more, so it gets as many columns as it can use. */
  .event-fields {
    --col-min: 11rem;
  }
  .macmahon-grid {
    --col-min: 26rem;
  }
  .rules-grid {
    --col-min: 20rem;
  }
  .other-grid {
    --col-min: 18rem;
  }
  /* Subsections. `fieldset`/`legend` rather than a `div` and a heading: these are
     genuine control groups (several are radio groups), and it is what names them
     to a screen reader. The browser's own chrome is reset; `min-width: 0`
     overrides the `min-content` floor a fieldset would otherwise impose as a grid
     item, which would stop the tracks from shrinking. */
  fieldset.sub {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  fieldset.sub > legend {
    padding: 0;
    margin: 0 0 0.3rem;
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--text-strong);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .field > span {
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  .field input {
    box-sizing: border-box;
    width: 100%;
    min-width: 0;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .preview-details {
    margin-top: 0.9rem;
  }
  .preview-details summary {
    cursor: pointer;
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  /* What the export's header will look like, updated as the fields are typed. */
  .header-preview {
    margin: 0.35rem 0 0;
    padding: 0.5rem 0.6rem;
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    font-size: 0.8rem;
    overflow-x: auto;
    white-space: pre;
    color: var(--text-secondary);
  }
  /* The description of the selected tournament mode, between the radios and the
     controls that mode adds. */
  .mode-desc {
    margin: 0.7rem 0 0.5rem;
  }
  .estimator {
    margin-top: 0.7rem;
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
  .thresholds {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-items: flex-start;
  }
  /* Wraps rather than overflowing: a threshold row is the widest control here and
     a narrow window (or a narrow track) has less than its full width. */
  .threshold-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .categories {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    align-items: flex-start;
  }
  .category-row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .category-name {
    width: 14rem;
    max-width: 100%;
    box-sizing: border-box;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
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
    margin: 0.8rem 0 0 1.2rem;
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
  /* The "add a tie-break" picker: its options carry the full description (DC's is
     very long). Without a definite width the select's flex-basis is its
     max-content, which stretches the row past the page. A definite width pins it;
     min-width:0 lets it shrink. The open option list still shows each description
     in full. Scoped to its own row so the compact ELO-knob selects keep their
     content width. */
  .tb-add {
    align-self: stretch;
  }
  .tb-add .tb-select {
    min-width: 0;
    width: 100%;
    max-width: 40rem;
  }
  .club-input {
    width: 12rem;
    max-width: 100%;
    box-sizing: border-box;
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
