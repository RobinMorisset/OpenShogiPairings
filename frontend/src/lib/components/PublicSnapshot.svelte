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
  import TabStrip from "./TabStrip.svelte";

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
    <!-- The same strip the live page renders, given files instead of a click
         handler — it is what makes these tabs links, and the only thing that
         does. The reader should not be able to tell that one is a script and
         the other a directory of files. -->
    <TabStrip
      tabs={sections.map((section) => ({
        id: sectionId(section),
        label: sectionLabel(section, $_),
        href: sectionFile(section, slug),
      }))}
      active={sectionId(current)}
    />
  {/if}

  <ContentCard wide={sectionNeedsWideCard(current)}>
    <PublicSectionBody {view} section={current} staticPage />
  </ContentCard>
</PageShell>

<!-- No styles left: the shell, the strip and the card are components now, and
     each brings its own — including the print rules this page used to have only
     half of. (It carried a partial copy of the card's screen rules and none of
     its print ones, so an exported standings table wide enough to matter was
     cropped at the edge of the sheet — in the one artifact whose whole point is
     being printed.) -->
