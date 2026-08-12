<!-- The event's identity: what the American Grid export's header carries. -->
<script lang="ts">
  import { _ } from "svelte-i18n";

  interface Props {
    /** The event's identity (empty = not entered). The date fields are
     *  `type="date"`, so their value is already the ISO `YYYY-MM-DD` the server
     *  accepts — and comparing two of them as strings compares them
     *  chronologically. */
    city: string;
    country: string;
    firstDate: string;
    lastDate: string;
    timeControl: string;
    /** The tournament's name, shown in the header preview. */
    tournamentName: string;
    busy: boolean;
    /** Send the settings as they now stand (the parent's single save path). */
    persist: () => void;
  }

  let {
    city = $bindable(),
    country = $bindable(),
    firstDate = $bindable(),
    lastDate = $bindable(),
    timeControl = $bindable(),
    tournamentName,
    busy,
    persist,
  }: Props = $props();

  function editCity(raw: string) {
    city = raw;
    persist();
  }

  function editCountry(raw: string) {
    country = raw;
    persist();
  }

  // The two dates are one setting: the header needs both the range and its
  // closing date, so entering one fills the other in (visibly, in the input) —
  // the one-day case — and clearing one clears the pair. A last day earlier than
  // the first is likewise pulled back into order rather than sent for the server
  // to reject.
  function editFirstDate(raw: string) {
    firstDate = raw;
    if (!raw) lastDate = "";
    else if (!lastDate || lastDate < raw) lastDate = raw;
    persist();
  }

  function editLastDate(raw: string) {
    lastDate = raw;
    if (!raw) firstDate = "";
    else if (!firstDate || firstDate > raw) firstDate = raw;
    persist();
  }

  function editTimeControl(raw: string) {
    timeControl = raw;
    persist();
  }

  // The header the American Grid export will carry, shown live under the fields
  // so the referee sees what FESA receives. Mirrors `header_lines` in
  // `american_grid.rs`; keep the two in step.
  const headerPreview = $derived.by(() => {
    const range = !firstDate || !lastDate
      ? null
      : firstDate === lastDate
        ? firstDate
        : `${firstDate} to ${lastDate}`;
    const parts = [tournamentName, city.trim(), country.trim(), range].filter(Boolean);
    const lines = [`[${parts.join(", ")}]`];
    if (range) lines.push(`[${lastDate}]`);
    const tc = timeControl.trim();
    if (tc) lines.push(`[Time control: ${tc}]`);
    return lines.join("\n");
  });
</script>

<section class="section">
  <h3>{$_("settings.eventTitle")}</h3>
  <p class="desc">{$_("settings.eventDesc")}</p>
  <div class="grid event-fields">
    <label class="field">
      <span>{$_("settings.eventCity")}</span>
      <input
        type="text"
        value={city}
        disabled={busy}
        onchange={(e) => editCity(e.currentTarget.value)}
      />
    </label>
    <label class="field">
      <span>{$_("settings.eventCountry")}</span>
      <input
        type="text"
        value={country}
        disabled={busy}
        onchange={(e) => editCountry(e.currentTarget.value)}
      />
    </label>
    <label class="field">
      <span>{$_("settings.eventFirstDay")}</span>
      <input
        type="date"
        value={firstDate}
        disabled={busy}
        onchange={(e) => editFirstDate(e.currentTarget.value)}
      />
    </label>
    <label class="field">
      <span>{$_("settings.eventLastDay")}</span>
      <input
        type="date"
        value={lastDate}
        disabled={busy}
        onchange={(e) => editLastDate(e.currentTarget.value)}
      />
    </label>
    <label class="field">
      <span>{$_("settings.eventTimeControl")}</span>
      <input
        type="text"
        value={timeControl}
        placeholder={$_("settings.eventTimeControlPlaceholder")}
        disabled={busy}
        onchange={(e) => editTimeControl(e.currentTarget.value)}
      />
    </label>
  </div>
  <!-- Collapsed: it is a preview of an export detail, not something to keep
       an eye on while filling the fields in. -->
  <details class="preview-details">
    <summary>{$_("settings.eventHeaderPreview")}</summary>
    <pre class="header-preview">{headerPreview}</pre>
  </details>
</section>

<style>
  /* Per-section track width: enough for this section's widest control, and no
     more, so it gets as many columns as it can use. */
  .event-fields {
    --col-min: 11rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .field > span {
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  .field input {
    box-sizing: border-box;
    width: 100%;
    min-width: 0;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .preview-details {
    margin-top: 0.9rem;
  }
  .preview-details summary {
    cursor: pointer;
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  /* What the export's header will look like, updated as the fields are typed. */
  .header-preview {
    margin: 0.35rem 0 0;
    padding: 0.5rem 0.6rem;
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    font-size: 0.8rem;
    overflow-x: auto;
    white-space: pre;
    color: var(--text-secondary);
  }
</style>
