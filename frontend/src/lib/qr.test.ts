import { describe, expect, it } from "vitest";
import { QR_QUIET_ZONE, qrMatrix, qrPath, qrSideModules } from "./qr";

/** A capability URL of the shape the publication panel actually encodes. */
const URL_UNDER_TEST =
  "https://osp.example/t/3f1c2b4a-5d6e-4f70-8091-a2b3c4d5e6f7/public?k=" +
  "c731c902f8c4fdf7c2b052c741debf6c0d37e71a8150e512";

describe("qrMatrix", () => {
  it("encodes a capability URL as a square matrix", async () => {
    const matrix = await qrMatrix(URL_UNDER_TEST);
    expect(matrix.length).toBeGreaterThan(0);
    for (const row of matrix) expect(row.length).toBe(matrix.length);
    // Versions are 21 + 4n modules a side; a ~110-character URL needs a middling
    // one, well short of the version-40 ceiling.
    expect((matrix.length - 21) % 4).toBe(0);
  });

  it("puts the three finder patterns where a scanner looks for them", async () => {
    const m = await qrMatrix(URL_UNDER_TEST);
    const last = m.length - 1;
    // A finder is a 7×7 dark ring; check the corner module and the light ring
    // one step in, at top-left, top-right and bottom-left — and that the fourth
    // corner has none, which is how a scanner works out the orientation.
    for (const [r, c] of [
      [0, 0],
      [0, last - 6],
      [last - 6, 0],
    ]) {
      expect(m[r][c]).toBe(true);
      expect(m[r + 1][c + 1]).toBe(false);
      expect(m[r + 3][c + 3]).toBe(true); // the 3×3 centre
    }
    expect(m[last][last]).toBe(false);
  });

  it("refuses to encode nothing rather than emitting an unreadable code", async () => {
    await expect(qrMatrix("")).rejects.toThrow();
  });

  it("reports a payload it cannot encode instead of truncating it", async () => {
    // Past what even a version-40 byte-mode code holds (~2,950 bytes at level M).
    await expect(qrMatrix("x".repeat(5000))).rejects.toThrow();
  });
});

describe("qrPath", () => {
  it("emits one 1×1 rectangle per dark module, offset by the quiet zone", async () => {
    const matrix = await qrMatrix(URL_UNDER_TEST);
    const dark = matrix.flat().filter(Boolean).length;
    const path = qrPath(matrix);
    expect(path.match(/M/g)?.length).toBe(dark);
    // The top-left finder's corner module sits at exactly the quiet-zone offset.
    expect(path.startsWith(`M${QR_QUIET_ZONE} ${QR_QUIET_ZONE}h1v1h-1z`)).toBe(true);
  });

  it("keeps every module inside the declared viewBox", async () => {
    const matrix = await qrMatrix(URL_UNDER_TEST);
    const side = qrSideModules(matrix);
    expect(side).toBe(matrix.length + 2 * QR_QUIET_ZONE);
    for (const [, x, y] of qrPath(matrix).matchAll(/M(\d+) (\d+)h/g)) {
      // +1 because each module is a 1×1 box drawn from its top-left corner.
      expect(Number(x) + 1).toBeLessThanOrEqual(side);
      expect(Number(y) + 1).toBeLessThanOrEqual(side);
    }
  });
});
