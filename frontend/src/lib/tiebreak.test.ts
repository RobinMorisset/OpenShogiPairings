import { describe, it, expect } from "vitest";
import { partitionDropped, sumDroppingLowest } from "./tiebreak";

const id = (v: number) => v;

describe("partitionDropped", () => {
  it("drops the `drop` lowest and keeps the rest, sorted ascending", () => {
    const { kept, dropped } = partitionDropped([2, 0, 1], id, 1);
    expect(dropped).toEqual([0]);
    expect(kept).toEqual([1, 2]);
  });

  it("drops nothing when drop is 0 (the plain sum)", () => {
    const { kept, dropped } = partitionDropped([2, 0, 1], id, 0);
    expect(dropped).toEqual([]);
    expect(kept).toEqual([0, 1, 2]);
  });

  it("drops the two lowest for a cut-2", () => {
    const { kept, dropped } = partitionDropped([3, 1, 2, 0], id, 2);
    expect(dropped).toEqual([0, 1]);
    expect(kept).toEqual([2, 3]);
  });

  it("keeps nothing when drop covers or exceeds the length", () => {
    expect(partitionDropped([1, 2], id, 2).kept).toEqual([]);
    expect(partitionDropped([1, 2], id, 5).kept).toEqual([]);
    expect(partitionDropped([1, 2], id, 5).dropped).toEqual([1, 2]);
  });

  it("treats a negative drop as zero", () => {
    expect(partitionDropped([2, 1], id, -1).kept).toEqual([1, 2]);
  });

  it("does not mutate the input array", () => {
    const input = [3, 1, 2];
    partitionDropped(input, id, 1);
    expect(input).toEqual([3, 1, 2]);
  });

  it("is stable among equal values (keeps input order)", () => {
    // Two items share value 5; the earlier one must stay first after sorting.
    const items = [
      { name: "a", v: 5 },
      { name: "b", v: 1 },
      { name: "c", v: 5 },
    ];
    const { kept } = partitionDropped(items, (t) => t.v, 1);
    expect(kept.map((t) => t.name)).toEqual(["a", "c"]);
  });
});

describe("sumDroppingLowest", () => {
  it("matches the server's Buchholz-cut definition", () => {
    // A's opponents scored {2, 1, 0}: cut-0 = 3, cut-1 drops the 0, cut-2 drops 0 and 1.
    expect(sumDroppingLowest([2, 1, 0], 0)).toBe(3);
    expect(sumDroppingLowest([2, 1, 0], 1)).toBe(3);
    expect(sumDroppingLowest([2, 1, 0], 2)).toBe(2);
  });

  it("the kept partition always sums to sumDroppingLowest", () => {
    const values = [4, 0, 2, 2, 7];
    for (let drop = 0; drop <= values.length + 1; drop++) {
      const { kept } = partitionDropped(values, id, drop);
      expect(kept.reduce((s, v) => s + v, 0)).toBe(sumDroppingLowest(values, drop));
    }
  });
});
