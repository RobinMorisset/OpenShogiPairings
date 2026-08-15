// QR codes for the public reader page (see docs/archive/public-access.md §3.2).
//
// The capability URL is ~100 characters of hex and a UUID: nothing anyone is
// going to type into a phone, so the QR code *is* the distribution mechanism,
// not a decoration next to the link.
//
// This wraps `qrcode-generator` (zero dependencies, MIT) down to the one thing
// the UI needs — a matrix of dark/light modules — so the rendering stays ours:
// the library's own `createSvgTag` bakes in colours and sizing, and the one
// thing that must not be themed here is the colour (see `QrCode.svelte`).

/** A QR code as a square grid of modules; `true` is a dark module. */
export type QrMatrix = boolean[][];

/**
 * The encoder, loaded on demand and then remembered.
 *
 * Dynamic because of who pays for it. The library is ~9 kB gzipped, and the
 * only screen that renders a QR code is the referee's publication panel —
 * opened perhaps once per tournament. The **reader** page loads the same
 * bundle, on a phone, on venue wifi, a hundred at a time, and never shows a
 * code at all. Static-importing it would put that weight on exactly the
 * population the rest of this feature works hardest to keep cheap.
 */
let encoder: Promise<typeof import("./qrEncoder")> | null = null;

/**
 * Encode `text` as a QR module matrix.
 *
 * Rejects if the text cannot be encoded — too long for even a version-40 code,
 * or containing characters the byte encoder can't represent. Callers surface
 * that rather than dropping the code silently: a publication panel showing a
 * link but no QR, with no explanation, is a referee standing at a wall
 * wondering what they did wrong.
 */
export async function qrMatrix(text: string): Promise<QrMatrix> {
  if (text === "") throw new Error("nothing to encode");
  encoder ??= import("./qrEncoder");
  return (await encoder).encode(text);
}

/**
 * The quiet zone, in modules, on each side of the code.
 *
 * Four is the spec's minimum, and it is not optional decoration: a scanner
 * finds the symbol by its contrast against this border, so a code butted up
 * against a table edge or a dark background often will not read at all.
 */
export const QR_QUIET_ZONE = 4;

/**
 * The matrix as one SVG path: every dark module a 1×1 rectangle, in module
 * coordinates offset by the quiet zone.
 *
 * One path rather than N rects because a version-7 code is ~1800 modules, and
 * that many DOM nodes is a visible cost for something rendered inside a panel
 * that opens and closes.
 */
export function qrPath(matrix: QrMatrix): string {
  const parts: string[] = [];
  for (let row = 0; row < matrix.length; row++) {
    for (let col = 0; col < matrix[row].length; col++) {
      if (matrix[row][col]) {
        parts.push(`M${col + QR_QUIET_ZONE} ${row + QR_QUIET_ZONE}h1v1h-1z`);
      }
    }
  }
  return parts.join("");
}

/** The side of the rendered code in modules, quiet zone included. */
export function qrSideModules(matrix: QrMatrix): number {
  return matrix.length + 2 * QR_QUIET_ZONE;
}
