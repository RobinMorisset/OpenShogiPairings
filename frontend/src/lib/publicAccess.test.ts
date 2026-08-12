import { describe, expect, it } from "vitest";
import { readPublicPage } from "./publicAccess";

const ID = "3f1c2b4a-5d6e-4f70-8091-a2b3c4d5e6f7";

describe("readPublicPage", () => {
  it("reads the tournament and key out of a capability URL", () => {
    expect(readPublicPage(new URL(`https://osp.example/t/${ID}/public?k=abc123`))).toEqual({
      id: ID,
      key: "abc123",
    });
  });

  it("tolerates a trailing slash and extra query parameters", () => {
    expect(
      readPublicPage(new URL(`https://osp.example/t/${ID}/public/?k=abc123&utm=qr`)),
    ).toEqual({ id: ID, key: "abc123" });
  });

  it("is not reader mode without a key — a bare link grants nothing", () => {
    expect(readPublicPage(new URL(`https://osp.example/t/${ID}/public`))).toBeNull();
    expect(readPublicPage(new URL(`https://osp.example/t/${ID}/public?k=`))).toBeNull();
  });

  it("is not reader mode for the ordinary app", () => {
    expect(readPublicPage(new URL("https://osp.example/"))).toBeNull();
    expect(readPublicPage(new URL("https://osp.example/t/nope/public?k=abc"))).toBeNull();
    expect(readPublicPage(new URL(`https://osp.example/t/${ID}/publicx?k=abc`))).toBeNull();
  });
});
