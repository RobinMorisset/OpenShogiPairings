import { describe, expect, it } from "vitest";
import { get, writable } from "svelte/store";
import {
  readTournamentUrl,
  tournamentUrlPath,
  wireTournamentUrl,
  type UrlHost,
} from "./tournamentUrl";

const ID = "3f1c2b4a-5d6e-4f70-8091-a2b3c4d5e6f7";
const OTHER = "9e8d7c6b-5a49-4838-a716-050403020100";

describe("readTournamentUrl", () => {
  it("reads the id out of a tournament URL", () => {
    expect(readTournamentUrl(new URL(`https://osp.example/t/${ID}`))).toBe(ID);
  });

  it("tolerates a trailing slash and ignores query parameters", () => {
    expect(readTournamentUrl(new URL(`https://osp.example/t/${ID}/?x=1`))).toBe(ID);
  });

  it("is null for the picker and for paths that are not a tournament", () => {
    expect(readTournamentUrl(new URL("https://osp.example/"))).toBeNull();
    expect(readTournamentUrl(new URL("https://osp.example/t/nope"))).toBeNull();
    expect(readTournamentUrl(new URL(`https://osp.example/t/${ID}x`))).toBeNull();
  });

  it("is null for the reader page — /public must not open a referee session", () => {
    expect(readTournamentUrl(new URL(`https://osp.example/t/${ID}/public?k=abc`))).toBeNull();
  });
});

describe("tournamentUrlPath", () => {
  it("round-trips with readTournamentUrl", () => {
    const path = tournamentUrlPath(ID);
    expect(readTournamentUrl(new URL(`https://osp.example${path}`))).toBe(ID);
    expect(tournamentUrlPath(null)).toBe("/");
  });
});

/**
 * A fake `window` with a real history stack: `back()`/`forward()` move the
 * index and fire `popstate`, as a browser would.
 */
function fakeWindow(initialPath: string) {
  const entries = [initialPath];
  let index = 0;
  const listeners: Array<() => void> = [];
  const win: UrlHost & { back(): void; forward(): void; entries: string[] } = {
    location: {
      get href() {
        return `https://osp.example${entries[index]}`;
      },
    },
    history: {
      pushState: (_data, _unused, url) => {
        entries.splice(index + 1);
        entries.push(url);
        index += 1;
      },
      replaceState: (_data, _unused, url) => {
        entries[index] = url;
      },
    },
    addEventListener: (_type, listener) => {
      listeners.push(listener);
    },
    back() {
      expect(index).toBeGreaterThan(0);
      index -= 1;
      for (const l of listeners) l();
    },
    forward() {
      expect(index).toBeLessThan(entries.length - 1);
      index += 1;
      for (const l of listeners) l();
    },
    entries,
  };
  return win;
}

describe("wireTournamentUrl", () => {
  it("adopts the URL's id, overriding the localStorage seed", () => {
    const store = writable<string | null>(OTHER); // the shared "last opened" seed
    const win = fakeWindow(`/t/${ID}`);
    wireTournamentUrl(store, win);
    expect(get(store)).toBe(ID);
    expect(win.entries).toEqual([`/t/${ID}`]); // no rewrite, no extra entry
  });

  it("normalises a fresh tab at / to the seeded tournament without a history entry", () => {
    const store = writable<string | null>(ID);
    const win = fakeWindow("/");
    wireTournamentUrl(store, win);
    expect(win.entries).toEqual([`/t/${ID}`]); // replaceState, not pushState
  });

  it("leaves a fresh tab with no seed at the picker", () => {
    const store = writable<string | null>(null);
    const win = fakeWindow("/");
    wireTournamentUrl(store, win);
    expect(get(store)).toBeNull();
    expect(win.entries).toEqual(["/"]);
  });

  it("pushes a history entry when the picker opens a tournament, and on the way back", () => {
    const store = writable<string | null>(null);
    const win = fakeWindow("/");
    wireTournamentUrl(store, win);

    store.set(ID); // the picker's select()
    expect(win.entries).toEqual(["/", `/t/${ID}`]);

    store.set(null); // the "back to picker" button
    expect(win.entries).toEqual(["/", `/t/${ID}`, "/"]);
  });

  it("follows Back/Forward without pushing new entries", () => {
    const store = writable<string | null>(null);
    const win = fakeWindow("/");
    wireTournamentUrl(store, win);
    store.set(ID);
    store.set(null);

    win.back(); // -> /t/{id}
    expect(get(store)).toBe(ID);
    win.back(); // -> /
    expect(get(store)).toBeNull();
    win.forward(); // -> /t/{id}
    expect(get(store)).toBe(ID);
    // Nothing above may have pushed: the stack is exactly the two navigations.
    expect(win.entries).toEqual(["/", `/t/${ID}`, "/"]);
  });

  it("walks between two tournaments opened in sequence", () => {
    const store = writable<string | null>(ID);
    const win = fakeWindow(`/t/${ID}`);
    wireTournamentUrl(store, win);

    store.set(null); // back to the picker…
    store.set(OTHER); // …and open the other one
    expect(win.entries).toEqual([`/t/${ID}`, "/", `/t/${OTHER}`]);

    win.back();
    win.back();
    expect(get(store)).toBe(ID);
  });
});
