<script lang="ts">
  // The rounded panel the page's content sits in — one of them, used by all
  // three renderers: the referee app, the live reader page and the static
  // export.
  //
  // It exists because of `wide`. The plain `.card` box is four declarations in
  // app.css and would not need a component; the *wide* card is a contract
  // between the card, its children and the printer, and each of the three
  // shells used to carry its own version of it. They diverged exactly as
  // copies do: the reader page never got one at all, so the standings table
  // painted its zebra rows past the rounded border onto the page background,
  // and the export got the first generation of the idiom and kept it — missing
  // `box-sizing`, which made every exported standings page ~50px wider than
  // its own viewport, and missing the print reset, in the one artifact whose
  // whole purpose is being printed and pinned to a wall.
  //
  // **The contract**, in one line: inside a `wide` card, only an element that
  // is itself a table may set the width — everything else is `.fit-width`
  // (app.css, where the reasoning lives).
  import type { Snippet } from "svelte";

  interface Props {
    /**
     * Grow the card behind a table that is wider than the screen.
     *
     * Deliberately a decision per section rather than always on: the card is
     * then sized with `max-content`, which asks its content how wide it would
     * like to be with nothing wrapped. For a table that is its natural width;
     * for a paragraph it is the whole sentence on one line.
     */
    wide?: boolean;
    children: Snippet;
  }

  let { wide = false, children }: Props = $props();
</script>

<section class="card" class:wide-table={wide}>
  {@render children()}
</section>

<style>
  /* A standings table can be far wider than the screen — a column per round
     plus the tie-breaks — and it is deliberately not wrapped in a scroller,
     because that would pin its sticky header to the scroller instead of to the
     window (see ResultsView). So it overflows, and the card grows to stay
     behind it rather than letting the table hang out of the rounded box. */
  .card.wide-table {
    width: max-content;
    min-width: 100%;
    /* The same trap `.fit-width` exists for, one level up: that `min-width` is
       a percentage, so without this it floors the *content* box at the page
       width and the card's own padding and border — 3rem and 2px — land
       outside it. Every tab with this class was that much wider than every tab
       without, whatever it contained, which is why they all came out to
       exactly the same width; and the static export, which never got this
       line, showed a permanent horizontal scrollbar on a page with nothing to
       scroll to. `max-content` is a keyword, not a length, so `box-sizing`
       leaves it alone (css-sizing-3 §5.1): a table that really is wider than
       the page still widens the card, padding outside it, as before. */
    box-sizing: border-box;
  }

  @media print {
    /* Ink on white: the box was there to hold the content together on a screen
       full of other things, and on paper the sheet already does that. */
    .card {
      border: none;
      background: transparent;
      padding: 0;
    }
    /* On screen the card grows past the viewport so the table has a background
       under it. On paper there is nothing to scroll and nothing to overflow
       *of* — a card sized to the table would simply run off the sheet and be
       cropped, so it goes back to the page width and the table lays itself out
       inside it. (`.fit-width` has the matching reset, in app.css.) */
    .card.wide-table {
      width: auto;
      min-width: 0;
    }
  }
</style>
