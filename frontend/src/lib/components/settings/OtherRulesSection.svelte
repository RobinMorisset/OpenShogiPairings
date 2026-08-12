<!-- The rules that belong to no pairing family: handicaps, absences, the
     player categories, and long boards. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { PlayerCategory } from "../../types";
  import type { HandicapChoice } from "../../handicap";

  interface Props {
    handicapPolicy: HandicapChoice;
    handicapWielRule: boolean;
    halfPointAbsences: boolean;
    /** Local editable rows (id + name), in entry order. */
    categories: PlayerCategory[];
    longBoardsEnabled: boolean;
    /** A team match is one board per player, so long boards are out there. */
    teamMode: boolean;
    busy: boolean;
    persist: () => void;
  }

  let {
    handicapPolicy = $bindable(),
    handicapWielRule = $bindable(),
    halfPointAbsences = $bindable(),
    categories = $bindable(),
    longBoardsEnabled = $bindable(),
    teamMode,
    busy,
    persist,
  }: Props = $props();

  function setHandicapPolicy(v: HandicapChoice) {
    handicapPolicy = v;
    persist();
  }

  function setHandicapWielRule(v: boolean) {
    handicapWielRule = v;
    persist();
  }

  function setHalfPointAbsences(v: boolean) {
    halfPointAbsences = v;
    persist();
  }

  function addCategory() {
    // A fresh client-minted id; the row persists once it gets a non-blank name
    // (a blank one is dropped by `cleanCategories`, so no request is sent yet).
    categories.push({ id: crypto.randomUUID(), name: "" });
  }

  function removeCategory(i: number) {
    categories.splice(i, 1);
    persist();
  }

  function editCategoryName(i: number, raw: string) {
    categories[i].name = raw;
    persist();
  }

  function setLongBoardsEnabled(v: boolean) {
    longBoardsEnabled = v;
    persist();
  }
</script>

<section class="section">
  <h3>{$_("settings.otherRulesTitle")}</h3>
  <div class="grid other-grid">
    <fieldset class="sub">
      <legend>{$_("settings.handicapTitle")}</legend>
      <p class="desc">
        {$_("settings.handicapDesc")}
      </p>
      <label class="check">
        <input
          type="radio"
          name="handicap-policy"
          value="none"
          checked={handicapPolicy === "none"}
          disabled={busy}
          onchange={() => setHandicapPolicy("none")}
        />
        {$_("settings.handicapNone")}
      </label>
      <label class="check">
        <input
          type="radio"
          name="handicap-policy"
          value="allowed"
          checked={handicapPolicy === "allowed"}
          disabled={busy}
          onchange={() => setHandicapPolicy("allowed")}
        />
        {$_("settings.handicapAllowed")}
      </label>
      <label class="check">
        <input
          type="radio"
          name="handicap-policy"
          value="suggested"
          checked={handicapPolicy === "suggested"}
          disabled={busy}
          onchange={() => setHandicapPolicy("suggested")}
        />
        {$_("settings.handicapSuggested")}
      </label>
      <label class="check">
        <input
          type="checkbox"
          checked={handicapWielRule}
          disabled={busy}
          onchange={(e) => setHandicapWielRule(e.currentTarget.checked)}
        />
        {$_("settings.handicapWielCheckbox")}
      </label>
      <p class="desc small-note">
        {$_("settings.handicapWielDesc")}
      </p>
    </fieldset>

    <fieldset class="sub">
      <legend>{$_("settings.absencesTitle")}</legend>
      <p class="desc">
        {$_("settings.absencesDesc")}
      </p>
      <label class="check">
        <input
          type="checkbox"
          checked={halfPointAbsences}
          disabled={busy}
          onchange={(e) => setHalfPointAbsences(e.currentTarget.checked)}
        />
        {$_("settings.halfPointAbsencesCheckbox")}
      </label>
      <p class="desc small-note">
        {$_("settings.halfPointAbsencesDesc")}
      </p>
    </fieldset>

    <fieldset class="sub">
      <legend>{$_("settings.categoriesTitle")}</legend>
      <p class="desc">
        {$_("settings.categoriesDesc")}
      </p>
      <div class="categories">
        {#each categories as row, i (row.id)}
          <div class="category-row">
            <input
              type="text"
              class="category-name"
              value={row.name}
              placeholder={$_("settings.categoryNamePlaceholder")}
              disabled={busy}
              onchange={(e) => editCategoryName(i, e.currentTarget.value)}
            />
            <button
              type="button"
              class="remove"
              disabled={busy}
              title={$_("settings.removeCategory")}
              onclick={() => removeCategory(i)}>✕</button
            >
          </div>
        {/each}
        {#if categories.length === 0}
          <p class="muted">{$_("settings.noCategories")}</p>
        {/if}
        <button
          type="button"
          class="ghost small"
          disabled={busy}
          onclick={addCategory}>{$_("settings.addCategory")}</button
        >
      </div>
    </fieldset>

    <!-- A team match is one board per player; a board spanning two rounds has
         no reading there, and the server rejects the pair. -->
    {#if !teamMode}
      <fieldset class="sub">
        <legend>{$_("settings.longBoardsTitle")}</legend>
        <p class="desc">
          {$_("settings.longBoardsDesc")}
        </p>
        <label class="check">
          <input
            type="checkbox"
            checked={longBoardsEnabled}
            disabled={busy}
            onchange={(e) => setLongBoardsEnabled(e.currentTarget.checked)}
          />
          {$_("settings.longBoardsCheckbox")}
        </label>
      </fieldset>
    {/if}
  </div>
</section>

<style>
  .other-grid {
    --col-min: 18rem;
  }
  .categories {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    align-items: flex-start;
  }
  .category-row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .category-name {
    width: 14rem;
    max-width: 100%;
    box-sizing: border-box;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
</style>
