<script lang="ts">
  // One section of the public page, for both of its readers: the live tab strip
  // (`PublicView`) and the exported file (`PublicSnapshot`).
  //
  // `publicSections` decides which sections exist; this decides what each one
  // *shows*. Both live in one place for the same reason: the two pages are read
  // side by side — someone in the room on their phone, someone at home on the
  // club's website — and a section that says one thing live and another in the
  // file is a disagreement nobody can resolve from either screen.
  //
  // The only difference between the two readers is `staticPage`, which drops the
  // controls no script will ever answer in an exported file (docs/public-access.md
  // §5). Everything else — which components, which props, what happens before
  // round 1 — is the same by construction.
  import { _ } from "svelte-i18n";
  import { handicapChoice } from "../handicap";
  import type { PublicSection } from "../publicPage";
  import type { PublicTournamentResponse } from "../types";
  import CupBracket from "./CupBracket.svelte";
  import EntrantList from "./EntrantList.svelte";
  import ResultsView from "./ResultsView.svelte";
  import RoundView from "./RoundView.svelte";

  interface Props {
    view: PublicTournamentResponse;
    /** The section to render — one of `publicSections(view)`. */
    section: PublicSection;
    /** Rendered once, to a file: no control that needs a script survives. */
    staticPage?: boolean;
  }

  let { view, section, staticPage = false }: Props = $props();

  const tournament = $derived(view.tournament);
  const teamMode = $derived(tournament.settings.teams != null);
</script>

{#if section.kind === "standings" && tournament.rounds.length === 0}
  <!-- Before round 1 there is nothing to rank: everyone sits at their MacMahon
       start. The entrant list is what the page is for at that moment — players
       check that they are registered (docs/public-access.md §2). -->
  <p class="muted">{$_("publicView.notStarted")}</p>
  <EntrantList players={tournament.players} />
{:else if section.kind === "standings"}
  <ResultsView
    tournament={{ ...tournament, draft: null }}
    standings={view.standings}
    teamStandings={teamMode ? (view.team_standings ?? []) : []}
    cupPodium={view.cup_podium ?? null}
    effectiveWinners={view.effective_winners ?? []}
    categories={tournament.settings.categories ?? []}
    {staticPage}
  />
  <!-- The two nullable fields are what `publicSections` gates the cup section on,
       so this narrowing never fails; it is here because the props need it. -->
{:else if section.kind === "cup" && tournament.cup && view.cup_bracket}
  <CupBracket
    bracket={view.cup_bracket}
    cup={tournament.cup}
    players={tournament.players}
  />
{:else if section.kind === "round"}
  <RoundView
    round={section.round.round}
    players={tournament.players}
    handicapPolicy={handicapChoice(tournament.settings.handicap_policy)}
    suggestedHandicaps={section.round.suggestedHandicaps}
    teams={teamMode ? (tournament.teams ?? []) : []}
    effectiveWinners={section.round.effectiveWinners}
    longEnabled={tournament.settings.long_boards_enabled}
    readOnly
    {staticPage}
  />
{/if}

<style>
  .muted {
    color: var(--text-secondary);
    text-align: center;
  }
</style>
