<script lang="ts">
  import { locale, _ } from "svelte-i18n";
  import { SUPPORTED_LOCALES, setLocale } from "../i18n";
</script>

<label class="locale-switcher">
  <span class="sr-only">{$_("localeSwitcher.label")}</span>
  <select class="control-xs control-quiet" value={$locale} onchange={(e) => setLocale(e.currentTarget.value)}>
    {#each SUPPORTED_LOCALES as l (l.code)}
      <option value={l.code}>{l.label}</option>
    {/each}
  </select>
</label>

<style>
  /* Raised rather than inset: it sits on the page header, not in a form. */
  .locale-switcher select {
    background: var(--bg-raised);
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
