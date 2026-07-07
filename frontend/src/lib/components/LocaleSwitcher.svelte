<script lang="ts">
  import { locale, _ } from "svelte-i18n";
  import { SUPPORTED_LOCALES, setLocale } from "../i18n";
</script>

<label class="locale-switcher">
  <span class="sr-only">{$_("localeSwitcher.label")}</span>
  <select value={$locale} onchange={(e) => setLocale(e.currentTarget.value)}>
    {#each SUPPORTED_LOCALES as l (l.code)}
      <option value={l.code}>{l.label}</option>
    {/each}
  </select>
</label>

<style>
  .locale-switcher select {
    background: var(--bg-raised);
    color: inherit;
    border: 1px solid var(--border);
    border-radius: 0.4rem;
    padding: 0.3rem 0.5rem;
    font: inherit;
    font-size: 0.8rem;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
  }
</style>
