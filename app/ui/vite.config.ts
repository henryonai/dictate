import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The Tauri CLI starts this dev server and points the window at it (`devUrl` in tauri.conf.json).
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    rollupOptions: {
      // Three pages: Settings, the one page of the floating dictation windows (`panel.html?view=bubble|card`), and the meetings window.
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        panel: fileURLToPath(new URL("./panel.html", import.meta.url)),
        meetings: fileURLToPath(new URL("./meetings.html", import.meta.url)),
      },
    },
  },
});
