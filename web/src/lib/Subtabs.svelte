<script>
  import { tick } from 'svelte';
  import { focusPane } from './ui.js';

  export let tab;
  export let items;
  export let active;
  export let closeLabel = 'Close';
  export let onselect;
  export let onclose = null;
  export let onmenu = null;

  function step(direction, wrap) {
    const index = items.findIndex((item) => item.id === active);
    const next = index + direction;
    if (next >= 0 && next < items.length) onselect(items[next].id);
    else if (wrap) onselect(items[(next + items.length) % items.length].id);
  }

  let strip;

  async function stripKey(event, item) {
    if (event.key === 'ArrowLeft') step(-1, false);
    else if (event.key === 'ArrowRight') step(1, false);
    else if (event.key === 'Delete' && onclose) onclose(item.id);
    else return;
    event.preventDefault();
    await tick();
    const selected = strip.querySelector('[aria-selected="true"]');
    if (selected) selected.focus();
  }

  async function pick(id) {
    onselect(id);
    await tick();
    focusPane(tab);
  }

  export function neighbour(id) {
    const index = items.findIndex((item) => item.id === id);
    const next = items[index + 1] ?? items[index - 1];
    return next ? next.id : null;
  }
</script>

{#if items.length}
  <div class="subtabs" role="tablist" bind:this={strip}>
    {#each items as item (item.id)}
      <span class="subtab" class:active={item.id === active} class:unread={item.unread}>
        <button
          class="select"
          role="tab"
          aria-selected={item.id === active}
          tabindex={item.id === active ? 0 : -1}
          on:click={() => pick(item.id)}
          on:auxclick={(e) => e.button === 1 && onclose && onclose(item.id)}
          on:keydown={(e) => stripKey(e, item)}
          on:contextmenu={(e) => onmenu && onmenu(e, item.id)}
        >
          <span class="name">{item.label}</span>
          {#if item.count !== undefined}<span class="count">{item.count}</span>{/if}
        </button>
        {#if onclose}
          <button
            class="close"
            tabindex="-1"
            title={closeLabel}
            aria-label="{closeLabel} {item.label}"
            on:click={() => onclose(item.id)}>×</button
          >
        {/if}
      </span>
    {/each}
  </div>
{/if}
