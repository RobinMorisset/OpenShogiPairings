<script lang="ts">
  import { _ } from "svelte-i18n";
  import { connectionStatus } from "../session";

  // Label per state; the dot colour is driven by the class.
  const labelKey = {
    online: "connection.live",
    connecting: "connection.reconnecting",
    offline: "connection.offline",
  } as const;
</script>

<div
  class="connection {$connectionStatus}"
  title={$_(`${labelKey[$connectionStatus]}Title`)}
>
  <span class="dot"></span>
  <span>{$_(labelKey[$connectionStatus])}</span>
</div>

<style>
  .connection {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.8rem;
    color: var(--text-secondary);
  }
  .dot {
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    background: var(--color-neutral);
    flex: none;
  }
  .connection.online .dot {
    background: var(--color-success);
  }
  .connection.connecting .dot {
    background: var(--color-warning);
  }
  .connection.offline .dot {
    background: var(--color-danger);
  }
</style>
