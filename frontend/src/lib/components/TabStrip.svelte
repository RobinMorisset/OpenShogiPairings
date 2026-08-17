<script lang="ts" module>
  /** One tab: what it says, what it is, and how it is reached. */
  export interface Tab {
    /** Identity — what `active` is matched against, and what `onSelect` gets. */
    id: string;
    label: string;
    /** Where this tab lives as a file. Only the static export sets it, and
     *  then every tab must have one: it is what makes the strip links. */
    href?: string;
    testid?: string;
    /** Stands out from the settled tabs — the round being prepared. */
    accent?: boolean;
  }
</script>

<script lang="ts">
  // The tab strip, for all three renderers.
  //
  // The static export's copy carried a comment saying its links "should be
  // indistinguishable" from the live pages' buttons; this makes that true by
  // construction rather than by two people editing two files the same way. The
  // export really does need different elements — a tab strip is a script, and
  // in a file where no script will ever run each section is its own page — so
  // the difference is `href` and nothing else. Everything about how a tab looks
  // is one rule here.
  import type { Snippet } from "svelte";

  interface Props {
    tabs: Tab[];
    /** The `id` of the tab showing. */
    active: string;
    /**
     * What clicking a tab does.
     *
     * Given, the strip is buttons; absent, every tab's `href` is followed
     * instead — and the tab you are on is plain text rather than a link
     * offering to reload the page you are already reading.
     */
    onSelect?: (id: string) => void;
    /** Anything else that belongs on the strip's line — the referee's
     *  round-lifecycle controls, which sit at its right end. */
    trailing?: Snippet;
  }

  let { tabs, active, onSelect, trailing }: Props = $props();
</script>

<!-- `fit-width` (app.css): a strip's max-content is every tab on one line, and
     inside a `wide` ContentCard only the table below may set the width. -->
{#if onSelect}
  <div class="tabs fit-width" role="tablist">
    {#each tabs as tab (tab.id)}
      <button
        type="button"
        class="tab"
        class:active={tab.id === active}
        class:accent={tab.accent}
        data-testid={tab.testid}
        onclick={() => onSelect(tab.id)}
      >
        {tab.label}
      </button>
    {/each}
    {@render trailing?.()}
  </div>
{:else}
  <nav class="tabs links fit-width">
    {#each tabs as tab (tab.id)}
      {#if tab.id === active}
        <span class="tab active" aria-current="page">{tab.label}</span>
      {:else}
        <a class="tab" class:accent={tab.accent} href={tab.href} data-testid={tab.testid}>
          {tab.label}
        </a>
      {/if}
    {/each}
    {@render trailing?.()}
  </nav>
{/if}

<style>
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-bottom: 1px solid var(--border);
    margin-bottom: 1.25rem;
  }
  /* Wider than the buttons' 0.25rem: a button gets a visible box from the
     browser, while a link here is bare text, and 4px of flex gap between two
     runs of text reads as one run of words. */
  .tabs.links {
    gap: 0.6rem;
  }
  .tab {
    padding: 0.4rem 0.8rem;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 0.4rem 0.4rem 0 0;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    text-decoration: none;
    cursor: pointer;
    /* Over the strip's own bottom border, so the selected tab's box opens into
       the panel below it. */
    margin-bottom: -1px;
  }
  .tab:hover:not(:disabled):not(.active) {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-color: var(--border);
    background: var(--bg-surface);
  }
  .tab.accent {
    font-style: italic;
    color: var(--color-accent);
  }

  @media print {
    /* Every renderer hides it: on the live pages a tab is a control, and in the
       export it is a link — and paper can follow neither. Whichever tab is open
       prints as the page. */
    .tabs {
      display: none;
    }
  }
</style>
