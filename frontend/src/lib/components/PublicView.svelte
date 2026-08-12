<script lang="ts">
  // The public reader page (docs/public-access.md §5): standings, the pairings
  // of every round played so far, and the cup bracket if there is one. No
  // settings, no player editing, no backups, no undo, no "why these pairings?".
  //
  // Everything here renders the *same* components the referee sees, from the
  // same server-computed standings — that is why the projection keeps the
  // `Tournament` shape instead of inventing a flat DTO. A second shape would
  // mean a second renderer to keep in sync, and the first time the two drifted
  // the wall display would quietly disagree with the referee's screen.
  //
  // The mode is decided by *what the server answered*, not by a flag set here:
  // the reader endpoint has no mutating handler behind it at all.
  import { _ } from "svelte-i18n";
  import { fetchPublicTournament, subscribeToPublicTournament } from "../api";
  import { describeApiError } from "../errorCodes";
  import { handicapChoice } from "../handicap";
  import type { PublicPage } from "../publicAccess";
  import type { PublicTournamentResponse, Round } from "../types";
  import ConnectionStatus from "./ConnectionStatus.svelte";
  import CupBracket from "./CupBracket.svelte";
  import LocaleSwitcher from "./LocaleSwitcher.svelte";
  import ResultsView from "./ResultsView.svelte";
  import RoundView from "./RoundView.svelte";
  import ThemeSwitcher from "./ThemeSwitcher.svelte";

  interface Props {
    page: PublicPage;
  }

  let { page }: Props = $props();

  let view = $state<PublicTournamentResponse | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  /** The tournament was deleted while this page was open. */
  let gone = $state(false);
  /** This link was revoked while the page was open — the referee issued a new
   *  one, or stopped publishing. Terminal: there is nothing to retry. */
  let revoked = $state(false);

  const tournament = $derived(view?.tournament ?? null);
  const rounds = $derived<Round[]>(tournament?.rounds ?? []);
  const teamMode = $derived(tournament?.settings.teams != null);
  const showResults = $derived(rounds.length > 0);
  const showCup = $derived(tournament?.cup != null);

  let activeTab = $state("results");
  const tabOrder = $derived([
    ...(showResults ? ["results"] : []),
    ...(showCup ? ["cup"] : []),
    ...rounds.map((r) => `round-${r.number}`),
  ]);
  const activeRound = $derived(
    rounds.find((r) => `round-${r.number}` === activeTab) ?? null,
  );

  // The suggested-handicap slice for the active round, matched by position —
  // empty unless the referee chose to publish the suggestion at all.
  const activeRoundSuggested = $derived.by(() => {
    if (!activeRound) return [];
    const idx = rounds.findIndex((r) => r.number === activeRound.number);
    return idx >= 0 ? ((view?.suggested_handicaps ?? [])[idx] ?? []) : [];
  });
  const activeRoundWinners = $derived.by(() => {
    if (!activeRound) return [];
    const idx = rounds.findIndex((r) => r.number === activeRound.number);
    return idx >= 0 ? ((view?.effective_winners ?? [])[idx] ?? []) : [];
  });

  // Land on the newest round — what someone in the room is looking for — and
  // keep the selection valid as rounds come and go (an undo or a backup restore
  // moves the public state *backwards*, which is correct: the referee has
  // decided the earlier state is the true one).
  $effect(() => {
    if (tabOrder.length === 0) return;
    if (!tabOrder.includes(activeTab)) {
      activeTab = tabOrder.at(-1) ?? "results";
    }
  });

  /**
   * Apply a payload, ignoring one that arrived out of order.
   *
   * The version is monotonic within a server run; a restart resets it, but a
   * restart also drops the stream, and the reconnect's first payload is the
   * authoritative one — so `force` lets it through unconditionally.
   */
  function apply(next: PublicTournamentResponse, force = false) {
    if (!force && view !== null && next.version < view.version) return;
    view = next;
    gone = false;
    error = null;
  }

  $effect(() => {
    const { id, key } = page;
    let cancelled = false;
    // The plain GET first: it is the cold-load path, it carries the `ETag` that
    // makes a refresh cheap, and it is what still shows a page if the stream is
    // refused because the tournament is at its reader cap.
    fetchPublicTournament(id, key)
      .then((first) => {
        if (!cancelled) apply(first);
      })
      .catch((err) => {
        if (!cancelled) error = describeApiError(err, $_);
      })
      .finally(() => {
        if (!cancelled) loading = false;
      });

    const unsubscribe = subscribeToPublicTournament(id, key, {
      // A reconnect may follow a server restart, whose version counter starts
      // over — so the stream's payload always wins.
      onState: (next) => apply(next, true),
      onGone: () => {
        gone = true;
      },
      onRevoked: () => {
        revoked = true;
      },
      onError: (message) => {
        error = message;
      },
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  });
</script>

<div class="app">
  <header>
    <div class="header-top">
      <div class="header-titles">
        <h1>{tournament?.name ?? "OpenShogiPairings"}</h1>
        <p class="subtitle">{$_("publicView.subtitle")}</p>
      </div>
      <div class="header-controls">
        {#if view}
          <ConnectionStatus />
        {/if}
        <ThemeSwitcher />
        <LocaleSwitcher />
      </div>
    </div>
  </header>

  {#if gone}
    <p class="error-banner" role="alert">{$_("publicView.gone")}</p>
  {:else if revoked}
    <!-- The standings below are the last state this link was entitled to, and
         they will not move again — say so, rather than leaving a page that
         looks live and silently isn't. -->
    <p class="error-banner" role="alert">{$_("publicView.revoked")}</p>
  {:else if error}
    <p class="error-banner" role="alert">{error}</p>
  {/if}

  {#if loading && !view}
    <p class="muted">{$_("app.loading")}</p>
  {:else if !view}
    <!-- No payload and no stream: a wrong, rotated, or never-issued key. The
         server answers the same 404 for all three on purpose, so this says the
         one thing that is certainly true. -->
    <section class="card">
      <p class="muted">{$_("publicView.unavailable")}</p>
    </section>
  {:else if tournament}
    <section class="card">
      {#if tabOrder.length === 0}
        <!-- Before round 1 there is nothing to rank: everyone sits at their
             MacMahon start. The entrant list is still worth showing — players
             check that they are registered. -->
        <p class="muted">{$_("publicView.notStarted")}</p>
        <ol class="entrants">
          {#each tournament.players as player (player.id)}
            <li>
              <span class="entrant-name">{player.last_name} {player.first_name}</span>
              {#if player.club}<span class="entrant-club">{player.club}</span>{/if}
              {#if player.rating != null}<span class="entrant-rating">{player.rating}</span>{/if}
            </li>
          {/each}
        </ol>
      {:else}
        <div class="tabs" role="tablist">
          {#if showResults}
            <button
              type="button"
              class="tab"
              class:active={activeTab === "results"}
              data-testid="public-tab-results"
              onclick={() => (activeTab = "results")}
            >
              {$_("app.tabResults")}
            </button>
          {/if}
          {#if showCup}
            <button
              type="button"
              class="tab"
              class:active={activeTab === "cup"}
              data-testid="public-tab-cup"
              onclick={() => (activeTab = "cup")}
            >
              {$_("app.tabCup")}
            </button>
          {/if}
          {#each rounds as round (round.number)}
            <button
              type="button"
              class="tab"
              class:active={activeTab === `round-${round.number}`}
              data-testid={`public-tab-round-${round.number}`}
              onclick={() => (activeTab = `round-${round.number}`)}
            >
              {round.completed
                ? $_("app.tabRoundCompleted", { values: { number: round.number } })
                : $_("app.tabRound", { values: { number: round.number } })}
            </button>
          {/each}
        </div>

        <div class="tab-content">
          {#if activeTab === "results"}
            <ResultsView
              tournament={{ ...tournament, draft: null }}
              standings={view.standings}
              teamStandings={teamMode ? (view.team_standings ?? []) : []}
              cupPodium={view.cup_podium ?? null}
              effectiveWinners={view.effective_winners ?? []}
              categories={tournament.settings.categories ?? []}
            />
          {:else if activeTab === "cup" && tournament.cup && view.cup_bracket}
            <CupBracket
              bracket={view.cup_bracket}
              cup={tournament.cup}
              players={tournament.players}
            />
          {:else if activeRound}
            <RoundView
              round={activeRound}
              players={tournament.players}
              handicapPolicy={handicapChoice(tournament.settings.handicap_policy)}
              suggestedHandicaps={activeRoundSuggested}
              teams={teamMode ? (tournament.teams ?? []) : []}
              effectiveWinners={activeRoundWinners}
              longEnabled={tournament.settings.long_boards_enabled}
              readOnly
            />
          {/if}
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .app {
    width: min(90rem, 95vw);
    margin: 0 auto;
    padding: 2rem 0 3rem;
  }
  header {
    margin-bottom: 1.5rem;
  }
  .header-top {
    display: flex;
    justify-content: center;
    align-items: flex-start;
    position: relative;
  }
  .header-titles {
    text-align: center;
  }
  .header-controls {
    position: absolute;
    right: 0;
    top: 0.2rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  h1 {
    font-size: 1.8rem;
    margin: 0;
  }
  .subtitle {
    color: var(--text-secondary);
    margin: 0.25rem 0 0;
  }
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-bottom: 1px solid var(--border);
    margin-bottom: 1.25rem;
  }
  .tab {
    padding: 0.4rem 0.8rem;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 0.4rem 0.4rem 0 0;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
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
  .entrants {
    margin: 1rem 0 0;
    padding-left: 1.5rem;
  }
  .entrants li {
    padding: 0.2rem 0;
    display: flex;
    gap: 0.75rem;
    align-items: baseline;
  }
  .entrant-name {
    font-weight: 600;
  }
  .entrant-club,
  .entrant-rating {
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  .error-banner {
    background: var(--bg-danger);
    border: 1px solid var(--border-danger);
    color: var(--text-on-danger);
    padding: 0.6rem 0.9rem;
    border-radius: 0.5rem;
    font-size: 0.9rem;
    margin-bottom: 1rem;
  }
  .muted {
    color: var(--text-secondary);
    text-align: center;
  }

  @media print {
    header,
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
