<script>
  import { drafts, sendMessage, status } from './stores.js';
  import { noAutofill } from './ui.js';
  import { onEnter } from './keys.js';

  export let key;
  export let placeholder;
  export let onsent;

  let input;
  let sending = false;
  let error = '';
  let errorKey = key;

  $: if (key !== errorKey) {
    errorKey = key;
    error = '';
  }
  $: text = $drafts[key] ?? '';

  export function focus() {
    input.focus();
  }

  function edit(event) {
    error = '';
    drafts.update((all) => ({ ...all, [key]: event.target.value }));
  }

  async function send() {
    if (sending || !$status.logged_in) return;
    sending = true;
    error = '';
    const sent = await sendMessage(key, (message) => (error = message));
    sending = false;
    if (sent) onsent();
  }
</script>

{#if error}
  <div class="notice">{error}</div>
{:else if !$status.logged_in}
  <div class="hint">Not connected to Soulseek. Your message stays here until you can send it.</div>
{/if}
<div class="toolbar">
  <input {...noAutofill}
    class="fill"
    data-primary
    {placeholder}
    value={text}
    bind:this={input}
    on:input={edit}
    on:keydown={onEnter(send)}
  />
  <button disabled={!$status.logged_in} on:mousedown|preventDefault on:click={send}>Send</button>
</div>
