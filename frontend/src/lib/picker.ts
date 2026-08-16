// Matching for the app's "pick one of these" comboboxes — the Teams tab's
// member picker and the round draft's forced pairing / forced bye fields.
//
// The widget itself is `components/Combobox.svelte`; the part worth testing on
// its own lives here, so the ranking rules can be checked without a DOM.

/** One thing a combobox can offer. */
export interface PickerOption {
  /** Identifies the choice to the caller, and the option's DOM id. Never "". */
  key: string;
  /** What is shown, and what the query is matched against. */
  label: string;
  /** Secondary text shown right-aligned — a rating, say. */
  meta?: string;
  /** Render `meta` in italics: a value that is not the official one. */
  metaUnofficial?: boolean;
}

/** Enough to choose from without the list covering what is under it. */
export const MAX_SUGGESTIONS = 8;

/** Lowercase + strip diacritics so "thune" matches "Thuné". */
export function normalize(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .trim();
}

/** Does any word of `label` start with `query`? Matching runs over the whole
 *  label, so "12. Dupont Jean" is found by the number, the surname or the given
 *  name; this is what decides which of those matches is offered first. */
function startsAWord(label: string, query: string): boolean {
  return normalize(label)
    .split(/\s+/)
    .some((word) => word.startsWith(query));
}

/**
 * The options an empty-or-typed-in query should offer, best first.
 *
 * An empty query lists the first `max` options as they came — the pool in its
 * own order, which is what "just show me what's left" means. Otherwise it keeps
 * the options whose label contains the query anywhere, and floats the ones
 * where it starts a word: typing "dup" wants Dupont before Ledup, and the sort
 * is stable, so everything else keeps the caller's order.
 */
export function matchOptions(
  options: PickerOption[],
  rawQuery: string,
  max: number = MAX_SUGGESTIONS,
): PickerOption[] {
  const query = normalize(rawQuery);
  if (query === "") return options.slice(0, max);
  const matches = options.filter((o) => normalize(o.label).includes(query));
  matches.sort((a, b) => (startsAWord(a.label, query) ? 0 : 1) - (startsAWord(b.label, query) ? 0 : 1));
  return matches.slice(0, max);
}
