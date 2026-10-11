<script>
  import { onDestroy } from 'svelte';
  import { noAutofill } from './ui.js';

  export let id;
  export let value;
  export let min;
  export let max = null;
  export let unit = '';
  export let locked = false;
  export let invalid = false;

  let text = String(value);
  let committed = value;

  $: if (value !== committed) {
    committed = value;
    text = String(value);
    invalid = false;
  }
  $: range = max === null ? `${min} or more` : `from ${min} to ${max}`;

  onDestroy(() => (invalid = false));

  function parsed() {
    const number = /^\d+$/.test(text.trim()) ? Number(text.trim()) : null;
    return number === null || number < min || (max !== null && number > max) ? null : number;
  }

  function typing() {
    const number = parsed();
    if (number === null) return;
    invalid = false;
    committed = number;
    value = number;
  }

  function commit() {
    const number = parsed();
    invalid = number === null;
    if (!invalid) text = String(number);
  }

  function keydown(event) {
    if (event.key === 'Enter') commit();
    else if (event.key === 'Escape') {
      text = String(value);
      invalid = false;
    }
  }
</script>

<input {...noAutofill}
  {id}
  class="number"
  class:invalid
  type="text"
  inputmode="numeric"
  readonly={locked}
  aria-invalid={invalid}
  bind:value={text}
  on:focus={(e) => e.target.select()}
  on:input={typing}
  on:blur={commit}
  on:keydown={keydown}
/>
{#if unit}<span class="unit">{unit}</span>{/if}
{#if invalid}<span class="notice">Enter a whole number {range}.</span>{/if}
