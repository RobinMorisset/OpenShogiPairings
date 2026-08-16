import { describe, expect, it } from "vitest";
import { matchOptions, normalize, type PickerOption } from "./picker";

function options(...labels: string[]): PickerOption[] {
  return labels.map((label, i) => ({ key: String(i), label }));
}

const labels = (found: PickerOption[]) => found.map((o) => o.label);

describe("normalize", () => {
  it("ignores case and diacritics", () => {
    expect(normalize("Thuné")).toBe("thune");
    expect(normalize("  ŁÓDŹ ")).toBe("łodz");
  });
});

describe("matchOptions", () => {
  it("offers the pool in its own order when nothing is typed", () => {
    const pool = options("3. Dupont Jean", "1. Abel Marie", "2. Zeller Ann");
    expect(labels(matchOptions(pool, ""))).toEqual([
      "3. Dupont Jean",
      "1. Abel Marie",
      "2. Zeller Ann",
    ]);
    // Whitespace is not a query — it must not empty the list.
    expect(labels(matchOptions(pool, "  "))).toHaveLength(3);
  });

  it("matches anywhere in the label, accents and case aside", () => {
    const pool = options("Thuné Erik", "Van Der Berg Jan");
    expect(labels(matchOptions(pool, "thune"))).toEqual(["Thuné Erik"]);
    expect(labels(matchOptions(pool, "BERG"))).toEqual(["Van Der Berg Jan"]);
    expect(matchOptions(pool, "zzz")).toEqual([]);
  });

  it("floats the options where the query starts a word", () => {
    const pool = options("Ledupont Marc", "Dupont Jean");
    expect(labels(matchOptions(pool, "dup"))).toEqual(["Dupont Jean", "Ledupont Marc"]);
  });

  it("finds a player by their tournament number", () => {
    // The round draft's labels start with it, and typing "12" must not be
    // outranked by the player whose surname merely contains those digits.
    const pool = options("7. Dupont Jean", "12. Abel Marie");
    expect(labels(matchOptions(pool, "12"))).toEqual(["12. Abel Marie"]);
  });

  it("keeps the caller's order among equally good matches", () => {
    const pool = options("Dupont Jean", "Dupond Marie", "Dupuis Ann");
    expect(labels(matchOptions(pool, "dup"))).toEqual([
      "Dupont Jean",
      "Dupond Marie",
      "Dupuis Ann",
    ]);
  });

  it("caps the list so it cannot cover the page", () => {
    const pool = options(...Array.from({ length: 30 }, (_, i) => `${i}. Dupont`));
    expect(matchOptions(pool, "")).toHaveLength(8);
    expect(matchOptions(pool, "dupont")).toHaveLength(8);
    expect(matchOptions(pool, "dupont", 3)).toHaveLength(3);
  });
});
