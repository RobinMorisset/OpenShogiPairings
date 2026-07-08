<script lang="ts">
  import { _ } from "svelte-i18n";
  import { ApiError, loginTournament } from "../api";

  interface Props {
    /** Called once the password is accepted and a token is stored. */
    onSuccess: () => void;
  }

  let { onSuccess }: Props = $props();

  let password = $state("");
  let busy = $state(false);
  let error = $state("");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (password.length === 0 || busy) return;
    busy = true;
    error = "";
    try {
      await loginTournament(password);
      password = "";
      onSuccess();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        error = $_("login.wrongPassword");
      } else if (err instanceof ApiError && err.status === 0) {
        error = $_("app.cannotReachServer");
      } else {
        error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      busy = false;
    }
  }
</script>

<section class="login card">
  <h2>{$_("login.title")}</h2>
  <p class="muted">{$_("login.prompt")}</p>
  <form onsubmit={submit}>
    <input
      type="password"
      bind:value={password}
      placeholder={$_("login.passwordPlaceholder")}
      autocomplete="current-password"
      disabled={busy}
    />
    <button type="submit" disabled={busy || password.length === 0}>
      {busy ? $_("login.signingIn") : $_("login.submit")}
    </button>
  </form>
  {#if error}
    <p class="error-banner" role="alert">{error}</p>
  {/if}
</section>

<style>
  .login {
    max-width: 26rem;
  }
  h2 {
    margin: 0 0 0.5rem;
    font-size: 1.15rem;
  }
  .muted {
    margin: 0 0 1rem;
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  form {
    display: flex;
    gap: 0.5rem;
  }
  input[type="password"] {
    flex: 1;
    box-sizing: border-box;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .error-banner {
    margin: 0.9rem 0 0;
    padding: 0.6rem 0.9rem;
    border-radius: 0.5rem;
    background: var(--bg-danger);
    border: 1px solid var(--border-danger);
    color: var(--text-on-danger);
    font-size: 0.9rem;
  }
</style>
