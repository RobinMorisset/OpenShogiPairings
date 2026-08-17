<script lang="ts">
  // One page of the static export (docs/archive/public-access.md phase 2).
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
  import {
    sectionFile,
    sectionId,
    sectionLabel,
    sectionNeedsWideCard,
    type PublicSection,
  } from "../publicPage";
  import type { PublicTournamentResponse } from "../types";
  import ContentCard from "./ContentCard.svelte";
  import PageShell from "./PageShell.svelte";
  import PublicSectionBody from "./PublicSectionBody.svelte";

  interface Props {
    view: PublicTournamentResponse;
    /** Every page of this export, for the navigation strip. */
    sections: PublicSection[];
    /** The one this file is. */
    current: PublicSection;
    /** The export's shared file-name stem, which the links between its pages
     *  are built from — every page of one export must sit in one directory. */
    slug: string;
    /** When the snapshot was taken, already formatted for the reader's locale
     *  by the caller — the exporter knows the locale, and a `Date` formatted
     *  here would be re-formatted on every render for no reason. */
    generatedAt: string;
  }

  let { view, sections, current, slug, generatedAt }: Props = $props();

  const tournament = $derived(view.tournament);
</script>

<!-- Not `publicView.subtitle`: that one ends in "— live", which is the one thing
     a file on somebody's web server is not. And `printHeader`, because the
     stamp under it is the whole reason this header exists. -->
<PageShell
  title={tournament.name}
  subtitle={$_("publicExport.subtitle")}
  stamp={$_("publicExport.generatedAt", { values: { when: generatedAt } })}
  printHeader
>
  {#if sections.length > 1}
    <nav class="tabs">
      {#each sections as section (sectionId(section))}
        {#if sectionId(section) === sectionId(current)}
          <!-- The page you are on is not a link to itself. It still looks like
               the selected tab; it just does not offer to reload the page you
               are already reading. -->
          <span class="tab active" aria-current="page">{sectionLabel(section, $_)}</span>
        {:else}
          <a class="tab" href={sectionFile(section, slug)}>{sectionLabel(section, $_)}</a>
        {/if}
      {/each}
    </nav>
  {/if}

  <ContentCard wide={sectionNeedsWideCard(current)}>
    <PublicSectionBody {view} section={current} staticPage />
  </ContentCard>
</PageShell>

<style>
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
  @media print {
    /* The links are the one thing paper cannot follow. */
    .tabs {
      display: none;
    }
    /* (The card's own print reset is `ContentCard`'s. This page used to have a
       partial copy of the card's screen rules and none of its print ones, so
       an exported standings table wide enough to matter was cropped at the
       edge of the sheet — in the one artifact whose whole point is being
       printed.) */
  }
</style>
