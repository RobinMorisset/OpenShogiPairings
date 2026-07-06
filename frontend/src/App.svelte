<script lang="ts">
  import { onMount } from "svelte";
  import { fetchHealth } from "./lib/api";
  import type { HealthStatus } from "./lib/types";

  type State =
    | { phase: "loading" }
    | { phase: "ok"; health: HealthStatus }
    | { phase: "error"; message: string };

  let state = $state<State>({ phase: "loading" });

  async function check() {
    state = { phase: "loading" };
    try {
      const health = await fetchHealth();
      state = { phase: "ok", health };
    } catch (err) {
      state = {
        phase: "error",
        message: err instanceof Error ? err.message : String(err),
      };
    }
  }

  onMount(check);
</script>

<main>
  <h1>OpenShogiPairings</h1>
  <p class="subtitle">Tournament management for shogi</p>

  <div class="card">
    <div class="status-line">
      <span class="dot {state.phase}"></span>
      {#if state.phase === "loading"}
        Contacting server…
      {:else if state.phase === "ok"}
        Server reachable
      {:else}
        Server unreachable
      {/if}
    </div>

    {#if state.phase === "ok"}
      <dl>
        <dt>status</dt>
        <dd>{state.health.status}</dd>
        <dt>service</dt>
        <dd>{state.health.service}</dd>
        <dt>version</dt>
        <dd>{state.health.version}</dd>
      </dl>
    {:else if state.phase === "error"}
      <p class="error-msg">{state.message}</p>
      <p class="error-msg">Is the server running? (<code>cargo run -p osp-server</code>)</p>
    {/if}

    <button onclick={check} disabled={state.phase === "loading"}>
      Re-check
    </button>
  </div>
</main>
