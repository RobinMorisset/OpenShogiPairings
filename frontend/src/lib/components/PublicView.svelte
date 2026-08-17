<script lang="ts">
  // The public reader page (docs/archive/public-access.md §5): standings, the pairings
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
  import type { PublicPage } from "../publicAccess";
  import { publicSections, sectionId, sectionLabel } from "../publicPage";
  import type { PublicTournamentResponse } from "../types";
  import ConnectionStatus from "./ConnectionStatus.svelte";
  import LocaleSwitcher from "./LocaleSwitcher.svelte";
  import PublicSectionBody from "./PublicSectionBody.svelte";
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

  // The tabs, and what each one shows, come from the same place as the static
  // export's pages — a tab this page has and that export lacks (or shows
  // differently) is a disagreement between two readers of the same tournament.
  const sections = $derived(view ? publicSections(view) : []);

  let activeTab = $state("standings");
  const activeSection = $derived(
    sections.find((s) => sectionId(s) === activeTab) ?? sections.at(-1) ?? null,
  );

  // Keep the selection valid as sections come and go: an undo or a backup
  // restore moves the public state *backwards*, which is correct — the referee
  // has decided the earlier state is the true one — and can take the round
  // being read out from under the reader. Standings is always there, so nobody
  // is ever left facing a page with no sections at all.
  $effect(() => {
    if (sections.length === 0) return;
    if (!sections.some((s) => sectionId(s) === activeTab)) {
      activeTab = sectionId(sections[sections.length - 1]);
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
  {:else if tournament && activeSection}
    <section class="card">
      <!-- One section is the whole page: before round 1 with no cup there is
           only the entrant list, and a tab strip of one tab is furniture. The
           export hides its navigation on the same condition. -->
      {#if sections.length > 1}
        <div class="tabs" role="tablist">
          {#each sections as section (sectionId(section))}
            <button
              type="button"
              class="tab"
              class:active={sectionId(section) === sectionId(activeSection)}
              data-testid={`public-tab-${sectionId(section)}`}
              onclick={() => (activeTab = sectionId(section))}
            >
              {sectionLabel(section, $_)}
            </button>
          {/each}
        </div>
      {/if}

      <div class="tab-content">
        <PublicSectionBody {view} section={activeSection} />
      </div>
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
  /* The referee app's header grid, ported verbatim — see the long comment on
     `.header-top` in App.svelte for why it is a grid. This page had the
     `position: absolute` version that comment describes, because it was written
     the same morning the fix landed and the fix only touched App.svelte. The
     failure is worse here than it ever was there: the centred cell holds the
     *tournament's own name*, so how much room it wants is up to the referee, and
     the controls are wider (ConnectionStatus says "Reconnecting…", not "Live").
     At a 560px window the controls sat 36px on top of the title.
       `align-items: start` rather than App's `center` is the one difference, and
     only because the middle cell is two lines here (name over subtitle): it
     keeps the controls level with the title, which is where `top: 0.2rem` used
     to put them. */
  .header-top {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: start;
    gap: 0.5rem;
  }
  .header-titles {
    grid-column: 2;
    text-align: center;
  }
  .header-controls {
    grid-column: 3;
    justify-self: end;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  /* Narrower than this the two would still fit side by side, but only by
     squeezing the title against them. Stack instead. */
  @media (max-width: 34rem) {
    .header-top {
      grid-template-columns: 1fr;
      justify-items: center;
    }
    .header-titles,
    .header-controls {
      grid-column: 1;
      justify-self: center;
    }
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
