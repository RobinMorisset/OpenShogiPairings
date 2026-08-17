import { mount } from "svelte";
import "./app.css";
import { prewarmApiBase } from "./lib/api";
import { waitLocale } from "./lib/i18n";
import { publicPage } from "./lib/publicAccess";
import App from "./App.svelte";
import PublicView from "./lib/components/PublicView.svelte";

// Locating the API server needs no locale, and the first thing the UI does
// once mounted is call it. Start that now so it overlaps with fetching the
// locale catalogue below instead of starting a fresh round-trip after the
// first render.
prewarmApiBase();

// Wait for the initial locale catalog to load before mounting: every
// component may call `$_` synchronously during its first render, and
// svelte-i18n throws if that happens before a locale is set.
const appPromise = waitLocale().then(() =>
  // A tab opened at a capability URL is a reader page and nothing else: no
  // picker, no login, no way back into the referee app (see
  // `lib/publicAccess.ts`). This is the only place that decides which of the
  // two the tab is.
  //
  // The reader page used to be mounted *by* App, behind an `{#if}` — which
  // rendered nothing of the referee app but still ran all of its effects, so
  // each one had to remember to stand down. The one that forgot would have
  // been invisible: opening a reader link in a browser that had a referee
  // session fetched and subscribed to *that* tournament, on that referee's
  // token, behind the reader's page.
  publicPage
    ? mount(PublicView, {
        target: document.getElementById("app")!,
        props: { page: publicPage },
      })
    : mount(App, {
        target: document.getElementById("app")!,
      }),
);

export default appPromise;
