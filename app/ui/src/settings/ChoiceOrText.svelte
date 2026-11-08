<script lang="ts">
  import { tick } from "svelte";
  import { t } from "../lib/i18n";
  import Field from "./Field.svelte";

  // A list to pick from that also takes a name that isn't on it. With no list, only the text field shows.
  let {
    label,
    customLabel,
    options,
    value,
    onchange,
  }: {
    label: string;
    /** Names the text field. */
    customLabel: string;
    options: { value: string; label: string }[];
    value: string;
    onchange: (value: string) => void;
  } = $props();

  // A value no option has can't be a real choice of the list.
  const OTHER = "\u0000other";

  let typing = $state(false);
  let text: HTMLInputElement | undefined = $state();

  const listed = $derived(options.some((option) => option.value === value));
  const showText = $derived(typing || !listed);

  async function pick(event: Event & { currentTarget: HTMLSelectElement }) {
    const picked = event.currentTarget.value;
    if (picked === OTHER) {
      typing = true;
      await tick();
      text?.focus();
    } else {
      typing = false;
      onchange(picked);
    }
  }

  function type(event: Event & { currentTarget: HTMLInputElement }) {
    const typed = event.currentTarget.value.trim();
    if (typed === "") return;
    typing = false;
    onchange(typed);
  }
</script>

{#if options.length > 0}
  <Field {label}>
    <select value={showText ? OTHER : value} onchange={pick}>
      {#each options as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
      <option value={OTHER}>{t("choice.other")}</option>
    </select>
  </Field>
{/if}
{#if showText}
  <Field label={customLabel}>
    <input bind:this={text} type="text" value={listed ? "" : value} onchange={type} />
  </Field>
{/if}
