import { mount } from "svelte";
import "./app.css";
import { waitLocale } from "./lib/i18n";
import App from "./App.svelte";

// Wait for the initial locale catalog to load before mounting: every
// component may call `$_` synchronously during its first render, and
// svelte-i18n throws if that happens before a locale is set.
const appPromise = waitLocale().then(() =>
  mount(App, {
    target: document.getElementById("app")!,
  }),
);

export default appPromise;
