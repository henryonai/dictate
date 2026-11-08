<script lang="ts">
  import { errorText } from "../lib/api";
  import { t } from "../lib/i18n";

  // A button that tests a connection. `run` resolves with the text to show on success; what it rejects with is
  // shown as it is.
  let { run }: { run: () => Promise<string> } = $props();

  let phase = $state<"idle" | "checking" | "ok" | "failed">("idle");
  let message = $state("");

  async function check() {
    phase = "checking";
    message = "";
    try {
      message = await run();
      phase = "ok";
    } catch (e) {
      message = errorText(e);
      phase = "failed";
    }
  }
</script>

<div class="check">
  <button type="button" onclick={check} disabled={phase === "checking"}>
    {phase === "checking" ? t("accounts.checking") : t("accounts.check")}
  </button>
  {#if phase === "ok"}
    <span class="result ok">{message}</span>
  {:else if phase === "failed"}
    <span class="result failed">{message}</span>
  {/if}
</div>

<style>
  .check {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 6px 0;
  }
  .result {
    font-size: 12px;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .ok {
    color: var(--ok);
  }
  .ok::before {
    content: "✓ ";
  }
  .failed {
    color: var(--danger);
  }
  .failed::before {
    content: "✗ ";
  }
</style>
