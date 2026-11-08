<script lang="ts">
  let {
    label,
    hint,
    checked,
    onchange,
  }: { label: string; hint?: string; checked: boolean; onchange: (checked: boolean) => void } = $props();
</script>

<div class="toggle">
  <label>
    <input type="checkbox" {checked} onchange={(event) => onchange(event.currentTarget.checked)} />
    <span>{label}</span>
  </label>
  {#if hint}
    <p class="hint">{hint}</p>
  {/if}
</div>
