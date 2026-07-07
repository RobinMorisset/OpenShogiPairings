<script lang="ts">
  import { _ } from "svelte-i18n";
  import { theme, setTheme } from "../theme";

  function toggle() {
    setTheme($theme === "dark" ? "light" : "dark");
  }
</script>

<button
  type="button"
  class="theme-switcher ghost"
  onclick={toggle}
  title={$_($theme === "dark" ? "themeSwitcher.switchToLight" : "themeSwitcher.switchToDark")}
  aria-label={$_($theme === "dark" ? "themeSwitcher.switchToLight" : "themeSwitcher.switchToDark")}
>
  {$theme === "dark" ? "🌙" : "☀️"}
</button>

<style>
  .theme-switcher {
    padding: 0.3rem 0.5rem;
    font-size: 0.9rem;
    line-height: 1;
  }
</style>
