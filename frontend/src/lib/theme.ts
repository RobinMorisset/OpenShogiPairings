import { writable } from "svelte/store";

export type Theme = "dark" | "light";

const STORAGE_KEY = "osp-theme";

function getInitialTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "dark" || stored === "light") return stored;

  // Light is the default regardless of OS preference — deliberate choice,
  // not a fallback signal like the locale's navigator.language check.
  return "light";
}

function apply(t: Theme) {
  document.documentElement.dataset.theme = t;
}

const initial = getInitialTheme();
apply(initial);

export const theme = writable<Theme>(initial);

/** Switch the active theme and persist the choice. */
export function setTheme(t: Theme) {
  localStorage.setItem(STORAGE_KEY, t);
  apply(t);
  theme.set(t);
}
