import { writable } from "svelte/store";

export const THEMES = ["dark", "light"] as const;
export type Theme = (typeof THEMES)[number];

const STORAGE_KEY = "osp-theme";

function getInitialTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "dark" || stored === "light") return stored;

  if (window.matchMedia("(prefers-color-scheme: light)").matches) return "light";

  return "dark";
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
