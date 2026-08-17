<script lang="ts">
  // The page itself: the centred column, the header above it and the footer
  // below — for all three renderers, the referee app, the live reader page and
  // the static export.
  //
  // It exists because the shell was written three times, and the copies drifted
  // in exactly the ways copies do. The header was a `position: absolute` block
  // that could land on top of the centred title; that was fixed in the referee
  // app, and the reader page — created the same morning from the pre-fix
  // version — kept the bug for two months, where it was worse (the centred text
  // is the tournament's own name, and the controls are wider). The print reset
  // for the column, which stops `95vw` resolving against the *sheet* and
  // indenting every printed page, existed in one copy of three.
  //
  // So the three differ now only in what they are given: a title, whether there
  // is a subtitle or a staleness stamp under it, what sits in the controls
  // column (nothing, in a file where no script will ever run), and whether the
  // header is part of the printed document.
  import type { Snippet } from "svelte";

  interface Props {
    /** The centred title: the app's name, or the tournament's. */
    title: string;
    /** A line under the title, in the same centred cell. */
    subtitle?: string;
    /** A line under that, already formatted: when a static page was taken. */
    stamp?: string;
    /** The right-hand column — theme, language, connection. Absent in the
     *  export, which has no script to answer any of them. */
    controls?: Snippet;
    /**
     * Keep the header on paper.
     *
     * Off for the two live pages: on screen it says which app you are looking
     * at, and on paper the sheet is about the tournament, not the software. The
     * export sets it, because its header carries the "taken at" stamp — a
     * printed static page with no date on it is the failure mode that whole
     * transport is written around.
     */
    printHeader?: boolean;
    /** Extra classes for the shell element, for a caller whose print jobs are
     *  states of the whole page (`printing-qr`, `printing-sheets`). */
    modifiers?: string;
    children: Snippet;
    /** Under the content, centred. */
    footer?: Snippet;
  }

  let {
    title,
    subtitle,
    stamp,
    controls,
    printHeader = false,
    modifiers = "",
    children,
    footer,
  }: Props = $props();
</script>

<div class="app {modifiers}">
  <header class:print-keep={printHeader}>
    <div class="header-top">
      <div class="header-titles">
        <h1>{title}</h1>
        {#if subtitle}
          <p class="subtitle">{subtitle}</p>
        {/if}
        {#if stamp}
          <p class="generated">{stamp}</p>
        {/if}
      </div>
      {#if controls}
        <div class="header-controls">
          {@render controls()}
        </div>
      {/if}
    </div>
  </header>

  {@render children()}

  {#if footer}
    <footer>
      {@render footer()}
    </footer>
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
  /* Three columns so the title is centred in the *window* while the controls
     still occupy real space at the right. They used to be `position: absolute`,
     which centred the title beautifully and let the controls sit on top of it
     the moment the window was too narrow for both — and no breakpoint fixes
     that, because the block's width depends on what is in it (the Live
     indicator only exists once a tournament is open). Laid out in the grid they
     cannot overlap at any width.

     `start` rather than `center` because the middle cell is two or three lines
     on the public pages, and centring against all of them would drop the
     controls to the middle of the block; this keeps them level with the title,
     which is where they were when they were absolutely positioned. On a
     one-line header it is the same row either way, give or take a pixel. */
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
    /* Level with the title rather than with the top of its cell: half the
       difference between the title's line box and this row of controls, which
       is what `align-items: center` gave the referee app back when its middle
       cell was the one-line one. Small enough that it barely reads as anything
       on the public pages, where the cell is two or three lines and centring
       against all of them would drop the controls to the middle of the block. */
    margin-top: 0.5rem;
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
    /* Stacked, the gap between the rows is the grid's to set. */
    .header-controls {
      margin-top: 0;
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
  .generated {
    color: var(--text-secondary);
    font-size: 0.9rem;
    margin: 0.5rem 0 0;
  }
  footer {
    margin-top: 2rem;
    display: flex;
    justify-content: center;
  }

  @media print {
    /* The page box is already the measure on paper, and there is no window to be
       95% of: `vw` resolves against the sheet, so `width: min(90rem, 95vw)` lays
       the content out at 95% of the page and `margin: 0 auto` splits the other
       5% into margins on top of the printer's own — indent on the left, lost
       width on the right, on a tab whose table needs every millimetre. The top
       padding pushes the first sheet down for nothing as well. */
    .app {
      width: auto;
      margin: 0;
      padding: 0;
    }
    header:not(.print-keep),
    footer {
      display: none;
    }
  }
</style>
