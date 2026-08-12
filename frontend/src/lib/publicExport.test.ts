// @vitest-environment jsdom
//
// The static export is the one thing in this codebase whose output nobody
// looks at before it is published: the referee saves a file and uploads it. So
// the properties that make it trustworthy are asserted here rather than left to
// be noticed on a club's website.

import { beforeAll, describe, expect, it } from "vitest";
import { addMessages, init, waitLocale } from "svelte-i18n";

import {
  buildDocument,
  collectStyles,
  renderPublicPages,
  renderSnapshotBody,
} from "./publicExport";
import { publicRounds, publicSections, sectionFile } from "./publicPage";
// The real catalogue, so a key this page uses but nobody translated shows up
// here as a missing string. `lib/i18n` itself is not imported: it reads
// `localStorage` and `navigator` to guess a language, which is browser
// behaviour this test has no opinion about.
import en from "./i18n/locales/en.json";
import type {
  Board,
  BracketMatch,
  Player,
  PublicTournamentResponse,
  Round,
  Standing,
} from "./types";

const player = (tid: number, last: string, rating: number | null): Player =>
  ({
    id: `p${tid}`,
    tournament_id: tid,
    last_name: last,
    first_name: "",
    rating,
    club: "Nantes",
    nationality: "FR",
    adjustments: [],
  }) as unknown as Player;

const standing = (tid: number, points: number): Standing =>
  ({
    player_id: `p${tid}`,
    victories: points,
    macmahon: 0,
    points,
    sosm: 0,
    sosw: 0,
    sodosm: 0,
    sodosw: 0,
    sososm: 0,
    sososw: 0,
    sosm1: 0,
    sosm2: 0,
    sosw1: 0,
    sosw2: 0,
    cussm: 0,
    cussw: 0,
    dc: 0,
    opponents: [],
    defeated: [],
    running_points: [],
    running_wins: [],
    estimated_elo: null,
  }) as unknown as Standing;

const board = (player1: number, player2: number): Board =>
  ({ player1, player2, outcome: { kind: "won", winner: "player1" } }) as Board;

const round = (number: number, boards: Board[]): Round =>
  ({
    number,
    boards,
    sitouts: [],
    completed: true,
    explanation: { round: number, boards: [], report: [] },
  }) as Round;

/** A two-player tournament with one played round. */
function view(rounds: Round[]): PublicTournamentResponse {
  return {
    tournament: {
      format_version: 5,
      id: "11111111-1111-1111-1111-111111111111",
      name: "Tournoi <de> Nantes",
      settings: {
        tiebreaks: ["points", "sos_m"],
        pairing: { kind: "macmahon" },
        handicap_policy: { kind: "none" },
        long_boards_enabled: false,
        categories: [],
        teams: null,
      },
      players: [player(1, "Miyamoto", 2100), player(2, "Duchesne", 1650)],
      registration_finalized: true,
      rounds,
      cup: null,
      teams: [],
    },
    version: 7,
    standings: [standing(1, 2), standing(2, 0)],
    team_standings: null,
    cup_podium: null,
    cup_bracket: null,
    suggested_handicaps: [],
    effective_winners: rounds.map((r) => r.boards.map(() => "player1" as const)),
  } as unknown as PublicTournamentResponse;
}

/**
 * The same tournament with a four-player cup seeded and nothing played yet —
 * the state between finalization (which freezes the bracket) and the first
 * confirmed round, which is where the live page and the export used to disagree.
 */
function withCup(v: PublicTournamentResponse): PublicTournamentResponse {
  const pending = (stage: BracketMatch["stage"]): BracketMatch => ({
    player1: null,
    player2: null,
    winner: null,
    stage,
  });
  v.tournament.players = [
    ...v.tournament.players,
    player(3, "Berger", 1900),
    player(4, "Nakada", 1800),
  ];
  v.tournament.cup = { size: 4, format: "direct", seed_order: [1, 2, 3, 4] };
  v.cup_bracket = {
    size: 4,
    qualification: null,
    rounds: [
      [
        { player1: 1, player2: 4, winner: null, stage: "semifinal" },
        { player1: 2, player2: 3, winner: null, stage: "semifinal" },
      ],
      [pending("final")],
    ],
    small_final: pending("small_final"),
  };
  return v;
}

beforeAll(async () => {
  addMessages("en", en);
  init({ fallbackLocale: "en", initialLocale: "en" });
  await waitLocale();
});

describe("publicRounds", () => {
  it("joins each round to its slice of the per-board arrays", () => {
    const v = view([round(1, [board(1, 2)]), round(2, [board(2, 1)])]);
    v.suggested_handicaps = [[null], ["h2" as never]];
    const sections = publicRounds(v);
    expect(sections.map((s) => s.round.number)).toEqual([1, 2]);
    expect(sections[1].suggestedHandicaps).toEqual(["h2"]);
    expect(sections[1].teamMatches).toEqual([]);
  });

  it("gives an empty slice when the projection omits the suggestions", () => {
    const v = view([round(1, [board(1, 2)])]);
    v.suggested_handicaps = [];
    expect(publicRounds(v)[0].suggestedHandicaps).toEqual([]);
  });
});

describe("publicSections", () => {
  it("is one page per tab, standings first", () => {
    const sections = publicSections(view([round(1, [board(1, 2)]), round(2, [board(2, 1)])]));
    expect(sections.map((s) => [s.kind, sectionFile(s, "cup")])).toEqual([
      ["standings", "cup.html"],
      ["round", "cup-round-1.html"],
      ["round", "cup-round-2.html"],
    ]);
  });

  it("is the standings page alone before round 1", () => {
    // Nothing to rank yet, and no round to look at: a navigation strip of one
    // link would be furniture.
    expect(publicSections(view([]))).toEqual([{ kind: "standings" }]);
  });

  it("has the cup page before round 1, alongside the entrant list", () => {
    // The bracket is frozen at finalization, so a cup with no round behind it is
    // what the room looks at while round 1 is being paired. Both sections are
    // reachable: the live page used to show only the bracket and the export only
    // the entrants, so neither reader saw what the other did.
    expect(publicSections(withCup(view([])))).toEqual([
      { kind: "standings" },
      { kind: "cup" },
    ]);
  });

  it("has no cup page when the projection carries no bracket", () => {
    // A cup with no `cup_bracket` would render an empty page; the section is
    // gated on both, which is what lets `PublicSectionBody` narrow to the props.
    const v = withCup(view([round(1, [board(1, 2)])]));
    v.cup_bracket = null;
    expect(publicSections(v).map((s) => s.kind)).toEqual(["standings", "round"]);
  });
});

/** Render one page of an export of `rounds`, by default the standings. */
function page(rounds: Round[], index = 0, when = "12 August 2026"): string {
  return pageOf(view(rounds), index, when);
}

/** Render one page of an export of `v`. */
function pageOf(v: PublicTournamentResponse, index = 0, when = "12 August 2026"): string {
  const sections = publicSections(v);
  return renderSnapshotBody(v, sections, sections[index], "cup", when);
}

describe("renderSnapshotBody", () => {
  it("renders a page by the real components", () => {
    const html = page([round(1, [board(1, 2)])]);
    expect(html).toContain("Miyamoto");
    expect(html).toContain("Duchesne");
    expect(html).toContain("12 August 2026");
    // The name is data, not markup: the angle brackets in the fixture's name
    // must arrive escaped.
    expect(html).toContain("Tournoi &lt;de&gt; Nantes");
    expect(html).not.toContain("Tournoi <de>");
  });

  it("navigates between its pages by plain links, but never to itself", () => {
    // The tab strip, without a script: the round page links back to the
    // standings, and shows its own tab as selected text rather than as a link
    // offering to reload the page you are reading.
    const html = page([round(1, [board(1, 2)])], 1);
    expect(html).toContain('href="cup.html"');
    expect(html).not.toContain('href="cup-round-1.html"');
    expect(html).toMatch(/<span class="tab[^"]*active[^"]*"[^>]*aria-current="page"/);
  });

  it("shows one section per page, not all of them", () => {
    const standings = page([round(1, [board(1, 2)])], 0);
    const roundOne = page([round(1, [board(1, 2)])], 1);
    // The standings table's header is on the standings page only; the board
    // table's is on the round page only.
    expect(standings).toContain("SOSM");
    expect(standings).not.toContain("Board");
    expect(roundOne).toContain("Board");
    expect(roundOne).not.toContain("SOSM");
  });

  it("leaves nothing clickable behind", () => {
    // The whole point of `staticPage`. No script will ever run in the exported
    // file, so a control that survived into it would be a button that silently
    // does nothing — on a page the referee has already uploaded.
    const html = page([round(1, [board(1, 2)])], 1);
    // Not a vacuous assertion: these are the very cells the referee's page
    // renders as clickable buttons, and they are here as plain spans.
    expect(html).toContain('class="player');
    expect(html).not.toContain("<button");
    expect(html).not.toContain("<input");
    expect(html).not.toContain("<select");
  });

  it("keeps the explanations as tooltips a static page can show", () => {
    const html = page([round(1, [board(1, 2)])]);
    // `data-tip` survives — it is what the CSS tooltip reads — but every
    // element carrying one must be marked, or nothing shows it.
    expect(html).toMatch(/data-tip="Sum of opponents' points/);
    // `osp-tip(?![-\w])` is the base class only, not its `-start`/`-end`
    // anchoring variants.
    expect(html.match(/data-tip=/g)!.length).toBe(html.match(/osp-tip(?![-\w])/g)!.length);
  });

  it("opens edge tooltips inwards, so none hangs off the page", () => {
    // The last tie-break column is at the right edge; a centred tooltip there
    // would widen the document and give it a horizontal scrollbar.
    const html = page([round(1, [board(1, 2)])]);
    expect(html).toContain("osp-tip-end");
  });

  it("says the tournament has not started when no round has", () => {
    const html = page([]);
    // The entrant list, not an empty standings table.
    expect(html).toContain("Miyamoto");
    expect(html).toContain("entrants");
  });

  it("publishes the bracket and the entrant list both, before round 1", () => {
    // The whole of C8: a cup exists from finalization, so at this moment there
    // are two things to look at and a reader must be able to reach either. The
    // export used to stop at the entrant list and the live page used to show
    // only the bracket.
    const cup = withCup(view([]));
    const entrants = pageOf(cup, 0);
    const bracket = pageOf(cup, 1);
    expect(entrants).toContain("entrants");
    expect(entrants).toContain('href="cup-cup.html"');
    // The bracket is drawn empty — every first-round pairing known, no winner
    // anywhere — and links back to the entrant list.
    expect(bracket).toContain("Nakada");
    expect(bracket).toContain('href="cup.html"');
  });

  it("leaves no host element behind in the document", () => {
    const before = document.body.childElementCount;
    page([round(1, [board(1, 2)])]);
    expect(document.body.childElementCount).toBe(before);
  });
});

describe("renderPublicPages", () => {
  it("names every page after the tournament, and titles them apart", () => {
    const files = renderPublicPages(
      view([round(1, [board(1, 2)])]),
      "en",
      ".x {}",
      new Date(2026, 7, 12, 12, 30),
    );
    expect(files.map((f) => f.fileName)).toEqual([
      "tournoi-de-nantes-public.html",
      "tournoi-de-nantes-public-round-1.html",
    ]);
    expect(files[0].contents).toContain("<title>Tournoi &lt;de&gt; Nantes</title>");
    expect(files[1].contents).toContain("<title>Tournoi &lt;de&gt; Nantes — Round 1 ✓</title>");
    // Every page is a whole document, and every page dates itself: a reader
    // may well be sent straight to round 2.
    for (const file of files) {
      expect(file.contents.startsWith("<!doctype html>")).toBe(true);
      expect(file.contents).toContain("August 12, 2026");
    }
  });
});

describe("collectStyles", () => {
  it("reads inline stylesheets out of the document", async () => {
    const style = document.createElement("style");
    style.textContent = ".public-marker { color: rebeccapurple; }";
    document.head.appendChild(style);
    try {
      expect(await collectStyles(document)).toContain("rebeccapurple");
    } finally {
      style.remove();
    }
  });
});

describe("buildDocument", () => {
  const doc = buildDocument({
    title: 'A "quoted" & <angled> name',
    lang: "fr",
    css: ".x { color: red }",
    body: "<p>hello</p>",
  });

  it("is a complete, self-contained document", () => {
    expect(doc.startsWith("<!doctype html>")).toBe(true);
    expect(doc).toContain('<html lang="fr">');
    expect(doc).toContain(".x { color: red }");
    expect(doc).toContain("<p>hello</p>");
    // Self-contained means no request may leave the page.
    expect(doc).not.toMatch(/<(script|link|img)\b/);
  });

  it("escapes the title rather than letting a tournament name write markup", () => {
    expect(doc).toContain("<title>A &quot;quoted&quot; &amp; &lt;angled&gt; name</title>");
  });

  it("keeps the file out of search engines, and out of dark mode", () => {
    expect(doc).toContain('<meta name="robots" content="noindex" />');
    expect(doc).not.toContain("data-theme");
  });
});
