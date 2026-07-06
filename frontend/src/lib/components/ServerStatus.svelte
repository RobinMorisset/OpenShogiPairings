<script lang="ts">
  import { onMount } from "svelte";
  import { fetchHealth } from "../api";
  import type { HealthStatus } from "../types";

  type State =
    | { phase: "loading" }
    | { phase: "ok"; health: HealthStatus }
    | { phase: "error"; message: string };

  let state = $state<State>({ phase: "loading" });

  async function check() {
    state = { phase: "loading" };
    try {
      state = { phase: "ok", health: await fetchHealth() };
    } catch (err) {
      state = {
        phase: "error",
        message: err instanceof Error ? err.message : String(err),
      };
    }
  }

  onMount(check);
</script>

<div class="server-status">
  <span class="dot {state.phase}"></span>
  {#if state.phase === "loading"}
    <span>Contacting server…</span>
  {:else if state.phase === "ok"}
    <span
      >Server <code>{state.health.service}</code> v{state.health.version}</span
    >
  {:else}
    <span>Server unreachable — <code>cargo run -p osp-server</code></span>
    <button onclick={check}>Retry</button>
  {/if}
</div>

<style>
  .server-status {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: #9a9aa2;
  }
  .dot {
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    background: #888;
    flex: none;
  }
  .dot.ok {
    background: #3fb950;
  }
  .dot.error {
    background: #f85149;
  }
  .dot.loading {
    background: #d29922;
  }
  button {
    font-size: 0.75rem;
    padding: 0.15rem 0.5rem;
  }
</style>
