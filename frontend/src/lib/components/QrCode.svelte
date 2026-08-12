<script lang="ts">
  // A QR code as inline SVG, so it scales to whatever the panel or the printer
  // gives it without ever resampling — which matters, because a QR code that
  // has been through a bitmap scale is exactly the kind of thing that scans
  // fine on the referee's screen and not at all from the wall.
  import { qrMatrix, qrPath, qrSideModules } from "../qr";

  interface Props {
    /** What the code encodes. */
    text: string;
    /** Accessible name — the code is an image, and "QR code" alone says
     *  nothing about which link it carries. */
    label: string;
  }

  let { text, label }: Props = $props();

  // The encoder is loaded on demand (see `qr.ts`), so this is a promise. A
  // failure is rendered rather than swallowed: the caller should be able to say
  // *why* there is no code instead of leaving a blank space next to the link.
  const encoded = $derived(
    qrMatrix(text).then((matrix) => ({
      path: qrPath(matrix),
      side: qrSideModules(matrix),
    })),
  );
</script>

{#await encoded}
  <!-- Nothing while the encoder loads: it is one small module off the same
       origin, so a spinner here would be a flash, not information. -->
{:then { path, side }}
  <!-- Deliberately *not* theme-aware. Reflectance reversal (light modules on a
       dark background) is optional in the QR spec, so plenty of phone scanners
       will not read an inverted code — and this one is read by a room full of
       phones nobody controls. Dark on white, in both themes, always. -->
  <svg
    class="qr"
    viewBox="0 0 {side} {side}"
    xmlns="http://www.w3.org/2000/svg"
    role="img"
    aria-label={label}
    shape-rendering="crispEdges"
  >
    <rect width={side} height={side} fill="#ffffff" />
    <path d={path} fill="#000000" />
  </svg>
{:catch error}
  <p class="qr-error" role="alert">{error.message}</p>
{/await}

<style>
  .qr {
    display: block;
    width: 100%;
    height: auto;
    /* The white quiet zone is part of the symbol, so the border sits outside
       it — a frame drawn any closer would eat the margin a scanner needs. */
    border: 1px solid var(--border);
    border-radius: 0.25rem;
  }
  .qr-error {
    margin: 0;
    font-size: 0.8rem;
    color: var(--color-warning);
  }
</style>
