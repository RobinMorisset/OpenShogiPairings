<script lang="ts">
  // One page of the static export (docs/public-access.md phase 2).
  //
  // Same projection and same leaf components as the live reader page — the
  // difference is that this one is rendered once, to a file, and no script will
  // ever run in it again. Two consequences shape it:
  //
  //   - **The tabs become links.** A tab strip is a script, and stacking every
  //     section into one long document (the first attempt) is unreadable at 45
  //     players and several rounds. So each section is its own file and the
  //     strip is a row of `<a href>` — which is what a tab strip was imitating
  //     in the first place. The pages of one export must therefore be saved
  //     side by side in one directory.
  //   - **No controls.** `staticPage` drops the toolbars and filters that would
  //     otherwise sit there looking clickable and doing nothing.
  //
  // The header states when the snapshot was taken, prominently, on every page.
  // A static page that has silently gone stale is the failure mode of this whole
  // transport: the referee re-exports after every round, and a reader who cannot
  // tell an old file from a live one will trust the wrong standings.
  import { _ } from "svelte-i18n";
  import { handicapChoice } from "../handicap";
  import { sectionLabel, type PublicSection } from "../publicPage";
  import type { PublicTournamentResponse } from "../types";
  import CupBracket from "./CupBracket.svelte";
  import EntrantList from "./EntrantList.svelte";
  import ResultsView from "./ResultsView.svelte";
  import RoundView from "./RoundView.svelte";

  interface Props {
    view: PublicTournamentResponse;
    /** Every page of this export, for the navigation strip. */
    sections: PublicSection[];
    /** The one this file is. */
    current: PublicSection;
    /** When the snapshot was taken, already formatted for the reader's locale
     *  by the caller — the exporter knows the locale, and a `Date` formatted
     *  here would be re-formatted on every render for no reason. */
    generatedAt: string;
  }

  let { view, sections, current, generatedAt }: Props = $props();

  const tournament = $derived(view.tournament);
  const teamMode = $derived(tournament.settings.teams != null);
</script>

<div class="app">
  <header>
    <h1>{tournament.name}</h1>
    <!-- Not `publicView.subtitle`: that one ends in "— live", which is the one
         thing a file on somebody's web server is not. -->
    <p class="subtitle">{$_("publicExport.subtitle")}</p>
    <p class="generated">{$_("publicExport.generatedAt", { values: { when: generatedAt } })}</p>
  </header>

  {#if sections.length > 1}
    <nav class="tabs">
      {#each sections as section (section.file)}
        {#if section.file === current.file}
          <!-- The page you are on is not a link to itself. It still looks like
               the selected tab; it just does not offer to reload the page you
               are already reading. -->
          <span class="tab active" aria-current="page">{sectionLabel(section, $_)}</span>
        {:else}
          <a class="tab" href={section.file}>{sectionLabel(section, $_)}</a>
        {/if}
      {/each}
    </nav>
  {/if}

  <section class="card">
    {#if current.kind === "standings" && tournament.rounds.length === 0}
      <!-- Before round 1 there is nothing to rank: everyone sits at their
           MacMahon start. The entrant list is still worth publishing — players
           check that they are registered. -->
      <p class="muted">{$_("publicView.notStarted")}</p>
      <EntrantList players={tournament.players} />
    {:else if current.kind === "standings"}
      <ResultsView
        tournament={{ ...tournament, draft: null }}
        standings={view.standings}
        teamStandings={teamMode ? (view.team_standings ?? []) : []}
        cupPodium={view.cup_podium ?? null}
        effectiveWinners={view.effective_winners ?? []}
        categories={tournament.settings.categories ?? []}
        staticPage
      />
    {:else if current.kind === "cup" && tournament.cup && view.cup_bracket}
      <CupBracket
        bracket={view.cup_bracket}
        cup={tournament.cup}
        players={tournament.players}
      />
    {:else if current.kind === "round"}
      <RoundView
        round={current.round.round}
        players={tournament.players}
        handicapPolicy={handicapChoice(tournament.settings.handicap_policy)}
        suggestedHandicaps={current.round.suggestedHandicaps}
        teams={teamMode ? (tournament.teams ?? []) : []}
        effectiveWinners={current.round.effectiveWinners}
        longEnabled={tournament.settings.long_boards_enabled}
        staticPage
      />
    {/if}
  </section>
</div>

<style>
  .app {
    width: min(90rem, 95vw);
    margin: 0 auto;
    padding: 2rem 0 3rem;
  }
  header {
    margin-bottom: 1.5rem;
    text-align: center;
  }
  h1 {
    font-size: 1.8rem;
    margin: 0;
  }
  .subtitle {
    color: var(--text-secondary);
    margin: 0.25rem 0 0;
  }
  .generated {
    color: var(--text-secondary);
    font-size: 0.9rem;
    margin: 0.5rem 0 0;
  }
  /* Deliberately the live page's tab styling, on anchors instead of buttons:
     the reader should not be able to tell that one is a script and the other a
     set of files. */
  .tabs {
    display: flex;
    flex-wrap: wrap;
    /* Wider than the live page's 0.25rem: there the tabs are buttons, which
       the browser gives a visible box; here they are bare text, and 4px of
       flex gap between two of those reads as one run of words. */
    gap: 0.6rem;
    border-bottom: 1px solid var(--border);
    margin-bottom: 1.25rem;
  }
  .tab {
    padding: 0.4rem 0.8rem;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 0.4rem 0.4rem 0 0;
    color: var(--text-secondary);
    text-decoration: none;
    font: inherit;
    margin-bottom: -1px;
  }
  .tab:hover:not(.active) {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-color: var(--border);
    background: var(--bg-surface);
  }
  .muted {
    color: var(--text-secondary);
    text-align: center;
  }

  @media print {
    /* The links are the one thing paper cannot follow. */
    .tabs {
      display: none;
    }
    .card {
      border: none;
      background: transparent;
      padding: 0;
    }
  }
</style>
