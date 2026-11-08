<script lang="ts">
  import { errorText } from "../lib/api";
  import { t } from "../lib/i18n";

  // A button that tests a connection. `run` resolves with the text to show on success; what it rejects with is
  // shown as it is. `config` stands for everything the test depends on (provider, URL, model, key): a result
  // is about the config it was got with and goes away when that changes.
  let { run, config }: { run: () => Promise<string>; config: string } = $props();

  let phase = $state<"idle" | "checking" | "ok" | "failed">("idle");
  let message = $state("");
  let checkedWith = "";

  async function check() {
    phase = "checking";
    message = "";
    let result: { phase: "ok" | "failed"; message: string };
    try {
      result = { phase: "ok", message: await run() };
    } catch (e) {
      result = { phase: "failed", message: errorText(e) };
    }
    // Whatever the run changed on its way (it saves a key that was typed) is part of this result.
    checkedWith = config;
    message = result.message;
    phase = result.phase;
  }

  $effect(() => {
    if ((phase === "ok" || phase === "failed") && config !== checkedWith) {
      phase = "idle";
      message = "";
    }
  });
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
