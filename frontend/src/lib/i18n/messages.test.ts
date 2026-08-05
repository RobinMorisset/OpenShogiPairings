// Checks the ICU messages themselves — that they say what the code expects and
// what each language needs. `check-i18n-keys.mjs` compares key *names*; nothing
// looked inside the values.
//
// A missing arm does not throw and does not look broken: `intl-messageformat`
// quietly falls back to `other`. In English that is invisible, because `other`
// is the only form a plural noun has. In Russian it renders "2 игроков" where
// the language wants "2 игрока" — grammatically wrong, plausible-looking, and
// unlikely to be reported. So the arms are checked here instead.
//
// The required set is derived from the counts a message can actually receive
// (integers), not from `Intl.PluralRules(...).pluralCategories`, which also
// lists categories only fractions can reach — Slovak's `many` is one, and
// demanding it would be noise. `other` is always required on top, because ICU
// itself mandates that arm.

import { IntlMessageFormat } from "intl-messageformat";
import { describe, expect, it } from "vitest";

const CATALOGUES = import.meta.glob("./locales/*.json", { eager: true }) as Record<
  string,
  { default: unknown }
>;

/** Plural categories reachable with a whole number of boards, points, players. */
function integerCategories(locale: string): Set<string> {
  const rules = new Intl.PluralRules(locale);
  const seen = new Set<string>(["other"]);
  for (let n = 0; n <= 200; n++) seen.add(rules.select(n));
  // Slavic rules key off the last one or two digits, so a few larger values
  // guard against a category that only appears past the loop above.
  for (const n of [1000, 1001, 1002, 1005, 1021, 1022, 1025]) seen.add(rules.select(n));
  return seen;
}

/** Every `{x, plural, …}` in a message, with the arms it declares. */
function pluralArms(message: string, locale: string): string[][] {
  const found: string[][] = [];
  const walk = (nodes: unknown[]) => {
    for (const node of nodes) {
      const n = node as { type?: number; options?: Record<string, { value: unknown[] }> };
      // Type 6 is `plural`; type 5 is `select`, which has no category rules.
      if (n.type === 6 && n.options) {
        found.push(Object.keys(n.options));
        for (const arm of Object.values(n.options)) walk(arm.value);
      } else if (n.options) {
        for (const arm of Object.values(n.options)) walk(arm.value);
      }
    }
  };
  walk(new IntlMessageFormat(message, locale).getAst());
  return found;
}

/**
 * The argument names a message interpolates, e.g. `{count}` → `count`.
 *
 * Read off the AST rather than by matching braces, because ICU uses braces for
 * plural arms too — a regex counts `other {# boards}` as an argument named
 * `other` and then reports every translation as mismatched.
 */
function argumentNames(message: string, locale: string): Set<string> {
  const names = new Set<string>();
  const walk = (nodes: unknown[]) => {
    for (const node of nodes) {
      const n = node as {
        type?: number;
        value?: unknown;
        options?: Record<string, { value: unknown[] }>;
      };
      // 0 is literal text, 7 is the `#` inside a plural arm; everything else
      // that carries a string `value` names an argument.
      if (n.type !== 0 && n.type !== 7 && typeof n.value === "string") names.add(n.value);
      if (n.options) for (const arm of Object.values(n.options)) walk(arm.value);
    }
  };
  walk(new IntlMessageFormat(message, locale).getAst());
  return names;
}

/** Every leaf string in a catalogue, with its dotted key. */
function entries(node: unknown, prefix = "", out: [string, string][] = []): [string, string][] {
  for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.push([path, value]);
    else if (value && typeof value === "object") entries(value, path, out);
  }
  return out;
}

const locales = Object.entries(CATALOGUES).map(([path, mod]) => ({
  locale: path.replace("./locales/", "").replace(".json", ""),
  catalogue: mod.default,
}));

describe("ICU plural messages", () => {
  it("finds catalogues to check", () => {
    expect(locales.length).toBeGreaterThan(1);
  });

  it.each(locales)("$locale declares every form the language needs", ({ locale, catalogue }) => {
    const required = integerCategories(locale);
    const problems: string[] = [];

    for (const [key, message] of entries(catalogue)) {
      if (!message.includes(", plural,")) continue;
      for (const declared of pluralArms(message, locale)) {
        const missing = [...required].filter((c) => !declared.includes(c));
        if (missing.length > 0) {
          problems.push(`${key}: missing ${missing.join(", ")} (has ${declared.join(", ")})`);
        }
      }
    }

    expect(problems, `these would silently fall back to the 'other' form`).toEqual([]);
  });

  // A message that parses but names the wrong argument renders the raw
  // placeholder, so check the plural actually selects on something.
  it.each(locales)("$locale has parseable plural messages", ({ locale, catalogue }) => {
    for (const [key, message] of entries(catalogue)) {
      if (!message.includes(", plural,")) continue;
      expect(() => new IntlMessageFormat(message, locale).getAst(), `${key} in ${locale}`).not.toThrow();
    }
  });
});

describe("interpolation arguments", () => {
  const en = locales.find((l) => l.locale === "en");
  if (!en) throw new Error("en.json is the reference and must be present");
  const reference = new Map(
    entries(en.catalogue).map(([key, msg]) => [key, argumentNames(msg, "en")]),
  );

  // A translation that drops `{count}` loses the number entirely; one that
  // invents `{cont}` renders the placeholder as literal text. Both look like
  // ordinary prose to anyone who doesn't read the language.
  it.each(locales.filter((l) => l.locale !== "en"))(
    "$locale interpolates exactly what English does",
    ({ locale, catalogue }) => {
      const problems: string[] = [];
      for (const [key, message] of entries(catalogue)) {
        const expected = reference.get(key);
        if (!expected) continue; // key parity is check-i18n-keys.mjs's job
        const actual = argumentNames(message, locale);
        const missing = [...expected].filter((a) => !actual.has(a));
        const unknown = [...actual].filter((a) => !expected.has(a));
        if (missing.length || unknown.length) {
          problems.push(
            `${key}: ${missing.length ? `dropped ${missing.join(", ")}` : ""}` +
              `${missing.length && unknown.length ? "; " : ""}` +
              `${unknown.length ? `invented ${unknown.join(", ")}` : ""}`,
          );
        }
      }
      expect(problems, "placeholders must match the English source").toEqual([]);
    },
  );
});
