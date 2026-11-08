<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  // Placeholder: the real settings window replaces this. It only shows what `get_settings` returns.
  let settings = $state<string>("Loading…");

  invoke("get_settings")
    .then((view) => {
      settings = JSON.stringify(view, null, 2);
    })
    .catch((error) => {
      settings = `Could not read the settings: ${error}`;
    });
</script>

<main>
  <h1>Dictate</h1>
  <pre>{settings}</pre>
</main>

<style>
  :global(html) {
    color-scheme: light dark;
  }
  :global(body) {
    margin: 0;
    font-family: system-ui, sans-serif;
    background: Canvas;
    color: CanvasText;
  }
  main {
    padding: 16px;
  }
  pre {
    white-space: pre-wrap;
    font-size: 12px;
  }
</style>
