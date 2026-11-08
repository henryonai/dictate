<script lang="ts">
  import type { Snippet } from "svelte";

  // A label on the left and its control on the right; clicking the label focuses the control.
  let { label, children }: { label: string; children: Snippet } = $props();
</script>

<label class="field">
  <span>{label}</span>
  <span class="field-control">{@render children()}</span>
</label>
