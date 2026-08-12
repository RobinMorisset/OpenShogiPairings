<script lang="ts">
  // The printable side of the result sheets (see `lib/resultSheets.ts` for what
  // they are for). Rendered only while a print is in flight, and visible only on
  // paper: App hides the rest of the page for the duration, so what comes out is
  // A4 sheets of slips to cut apart along the dashed lines.
  import { _ } from "svelte-i18n";
  import { formatGrade } from "../grade";
  import { formatScore } from "../score";
  import { paginate, sheetsPerPage, type SheetPlayer } from "../resultSheets";

  interface Props {
    tournamentName: string;
    /** One slip per player, in tournament-number order. */
    players: SheetPlayer[];
    /** How many rounds the tournament is planned to have: one row each. */
    rounds: number;
    /** Extra slips with an empty header, for players registering late. */
    blanks: number;
    /** Whether to print the MacMahon starting-points row (see
     *  `macMahonRowShown`); the players' `macmahon` is set exactly then. */
    macMahonRow: boolean;
  }

  let { tournamentName, players, rounds, blanks, macMahonRow }: Props = $props();

  // A `null` slip is a blank one — the same rows, with an empty header for the
  // referee to write a late arrival's name and number into by hand.
  const slips = $derived<(SheetPlayer | null)[]>([
    ...players,
    ...Array.from({ length: blanks }, () => null),
  ]);
  const perPage = $derived(sheetsPerPage(rounds + (macMahonRow ? 1 : 0)));
  const pages = $derived(paginate(slips, perPage));
  const roundNumbers = $derived(Array.from({ length: rounds }, (_v, i) => i + 1));

  // `macMahonRow` and every slip's `macmahon` are set by the same rule (see
  // `buildSheetPlayers`), so they cannot disagree — and if they ever do, the
  // print fails rather than hand a player a starting score of zero.
  function macMahonStart(slip: SheetPlayer): number {
    if (slip.macmahon === null) {
      throw new Error(`no MacMahon start for #${slip.tournamentId}, on a sheet that prints one`);
    }
    return slip.macmahon;
  }
</script>

<div class="result-sheets" data-testid="result-sheets">
  {#each pages as page, pageIndex (pageIndex)}
    <!-- One printed page per grid, so the breaks fall between slips rather than
         wherever a long flow happens to reach the bottom of the paper. -->
    <div
      class="page"
      class:four-up={perPage === 4}
      class:last={pageIndex === pages.length - 1}
    >
      {#each page as slip, slipIndex (slipIndex)}
        <section class="slip">
          <div class="tournament">{tournamentName}</div>
          <div class="who">
            <span class="tid rule">{slip ? slip.tournamentId : ""}</span>
            <span class="name rule">{slip ? slip.name : ""}</span>
          </div>
          <div class="meta">
            <span class="field"
              >{$_("resultSheets.elo")}
              <span class="rule">{slip?.rating ?? ""}</span></span
            >
            <span class="field"
              >{$_("resultSheets.grade")}
              <span class="rule">{slip?.grade ? formatGrade(slip.grade) : ""}</span></span
            >
          </div>
          <table>
            <colgroup>
              <col class="col-round" />
              <col class="col-board" />
              <col class="col-opponent" />
              <col class="col-points" />
            </colgroup>
            <thead>
              <tr>
                <th>{$_("resultSheets.roundColumn")}</th>
                <th>{$_("resultSheets.boardColumn")}</th>
                <th>{$_("resultSheets.opponentColumn")}</th>
                <th>{$_("resultSheets.pointsColumn")}</th>
              </tr>
            </thead>
            <tbody>
              {#if macMahonRow}
                <!-- The points a player starts on, before a single game: round
                     0, no board, and the group they were seeded into. -->
                <tr>
                  <td>0</td>
                  <td>{$_("resultSheets.na")}</td>
                  <td>{$_("resultSheets.macmahon")}</td>
                  <td>{slip ? formatScore(macMahonStart(slip)) : ""}</td>
                </tr>
              {/if}
              {#each roundNumbers as number (number)}
                <!-- Everything but the round number is filled in by hand: the
                     referee writes board and opponent, the player adds the
                     handicap and the result, the referee the new total. -->
                <tr>
                  <td>{number}</td>
                  <td></td>
                  <td></td>
                  <td></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/each}
    </div>
  {/each}
</div>

<style>
  /* Paper only: on screen this never shows, it is rendered for the duration of
     the print and thrown away again. */
  .result-sheets {
    display: none;
  }

  @media print {
    .result-sheets {
      display: block;
      color: #000;
      /* Pinned rather than inherited from the app, so the header heights below
         are predictable in `em`. */
      line-height: 1.2;
    }
    /* Six slips to an A4 (2 × 3 of 8.4cm), or four (2 × 2 of 12.6cm) once the
       round count would squeeze the rows too small — either way 25.2cm of the
       page, which clears the widest default print margin. */
    .page {
      display: grid;
      grid-template-columns: 1fr 1fr;
      grid-auto-rows: 8.4cm;
      break-after: page;
    }
    .page.four-up {
      grid-auto-rows: 12.6cm;
    }
    /* Without this the last page break emits a trailing blank sheet. */
    .page.last {
      break-after: auto;
    }
    .slip {
      box-sizing: border-box;
      /* The cut lines. */
      border: 1px dashed #999;
      padding: 0.3cm 0.35cm;
      display: flex;
      flex-direction: column;
      overflow: hidden;
    }
    .tournament {
      font-size: 7pt;
      color: #444;
      overflow: hidden;
      white-space: nowrap;
      text-overflow: ellipsis;
    }
    /* `flex-end`, not `baseline`: a blank slip's rules have no text to take a
       baseline from, and the rules would then sit at different heights from a
       filled slip's. Their bottom edges — the ruled lines themselves — are what
       should line up. */
    .who {
      display: flex;
      align-items: flex-end;
      gap: 0.25cm;
      margin-top: 0.1cm;
    }
    .tid {
      font-size: 13pt;
      font-weight: 700;
      min-width: 1cm;
      text-align: center;
    }
    .name {
      flex: 1;
      font-size: 10pt;
      font-weight: 600;
      overflow: hidden;
      white-space: nowrap;
    }
    .meta {
      display: flex;
      gap: 0.6cm;
      margin-top: 0.15cm;
      font-size: 8pt;
    }
    .field {
      display: flex;
      align-items: flex-end;
      gap: 0.15cm;
    }
    /* Every header value is a ruled blank carrying its value when we know it, so
       an unrated player — or a whole blank slip — has somewhere to write it.
       The height is fixed because an empty inline-block is zero-tall: without it
       a blank slip's header collapses and the table below, which stretches to
       fill the slip, grows into the space it left. */
    .rule {
      display: inline-block;
      min-width: 1.4cm;
      height: 1.25em;
      overflow: hidden;
      border-bottom: 1px solid #000;
    }
    table {
      flex: 1;
      width: 100%;
      margin-top: 0.2cm;
      border-collapse: collapse;
      table-layout: fixed;
      font-size: 8pt;
    }
    .col-round {
      width: 16%;
    }
    .col-board {
      width: 16%;
    }
    .col-opponent {
      width: 44%;
    }
    .col-points {
      width: 24%;
    }
    th,
    td {
      border: 1px solid #000;
      padding: 0 0.08cm;
      text-align: center;
    }
    th {
      font-size: 7pt;
      font-weight: 600;
    }
    /* A floor, not the final height: the table stretches to the bottom of the
       slip and the rows share out whatever is left over. */
    td {
      height: 0.5cm;
    }
  }
</style>
