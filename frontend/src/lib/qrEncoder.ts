// The QR library itself, behind a module of its own so `qr.ts` can load it on
// demand — see the note there on who pays for the bytes.
//
// Static-importing it *here*, rather than dynamic-importing the package
// directly from `qr.ts`, is also what keeps the typing honest: the package
// declares `export = qrcode`, so a dynamic `import("qrcode-generator")` types
// as the callable namespace and has no `.default` as far as TypeScript is
// concerned — even though the ESM build really does export one. Going through
// a default import sidesteps that without a cast.

import qrcode from "qrcode-generator";
import type { QrMatrix } from "./qr";

/**
 * Error-correction level. `M` (~15% recoverable) is the usual default and the
 * right trade here: the code is printed once and read across a room, where a
 * thumbtack through a corner or a bit of glare is likelier than the code being
 * too dense to resolve.
 */
const ERROR_CORRECTION = "M";

/** Encode `text`, throwing if it does not fit even a version-40 symbol. */
export function encode(text: string): QrMatrix {
  // Type number 0 = pick the smallest version the data fits in.
  const code = qrcode(0, ERROR_CORRECTION);
  code.addData(text);
  code.make();
  const size = code.getModuleCount();
  return Array.from({ length: size }, (_, row) =>
    Array.from({ length: size }, (_, col) => code.isDark(row, col)),
  );
}
