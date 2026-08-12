import { init, register, locale, waitLocale } from "svelte-i18n";

interface LocaleOption {
  code: string;
  label: string;
}

/** Adding a language: drop a new `locales/<code>.json` file, register it
 * below, and add it here — no other code needs to change. */
export const SUPPORTED_LOCALES: LocaleOption[] = [
  { code: "en", label: "English" },
  { code: "fr", label: "Français" },
  { code: "de", label: "Deutsch" },
  { code: "ja", label: "日本語" },
  { code: "ru", label: "Русский" },
  { code: "be", label: "Беларуская" },
  { code: "uk", label: "Українська" },
  { code: "sk", label: "Slovenčina" },
  { code: "pl", label: "Polski" },
];

const STORAGE_KEY = "osp-locale";

register("en", () => import("./locales/en.json"));
register("fr", () => import("./locales/fr.json"));
register("de", () => import("./locales/de.json"));
register("ja", () => import("./locales/ja.json"));
register("ru", () => import("./locales/ru.json"));
register("be", () => import("./locales/be.json"));
register("uk", () => import("./locales/uk.json"));
register("sk", () => import("./locales/sk.json"));
register("pl", () => import("./locales/pl.json"));

function isSupported(code: string): boolean {
  return SUPPORTED_LOCALES.some((l) => l.code === code);
}

function getInitialLocale(): string {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored && isSupported(stored)) return stored;

  const browserLocale = navigator.language.split("-")[0];
  if (isSupported(browserLocale)) return browserLocale;

  return "en";
}

init({
  fallbackLocale: "en",
  initialLocale: getInitialLocale(),
});

// Keep the document language in sync with the UI language, so screen readers,
// `:lang()` rules and browser hyphenation follow it instead of the hardcoded
// `lang="en"` in index.html.
locale.subscribe((code) => {
  if (code) document.documentElement.lang = code;
});

/** Switch the active UI language and persist the choice. */
export function setLocale(code: string) {
  if (!isSupported(code)) return;
  localStorage.setItem(STORAGE_KEY, code);
  locale.set(code);
}

export { waitLocale };
