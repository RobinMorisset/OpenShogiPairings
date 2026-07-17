import { mount } from "svelte";
import "./app.css";
import { prewarmApiBase } from "./lib/api";
import { waitLocale } from "./lib/i18n";
import App from "./App.svelte";

// Locating the API server needs no locale, and the first thing the UI does
// once mounted is call it. Start that now so it overlaps with fetching the
// locale catalogue below instead of starting a fresh round-trip after the
// first render.
prewarmApiBase();

// Wait for the initial locale catalog to load before mounting: every
// component may call `$_` synchronously during its first render, and
// svelte-i18n throws if that happens before a locale is set.
const appPromise = waitLocale().then(() =>
  mount(App, {
    target: document.getElementById("app")!,
  }),
);

export default appPromise;
