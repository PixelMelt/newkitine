<script>
  import { closeMenu } from './menu.js';

  export let items;

  function run(item) {
    if (item.disabled) return;
    closeMenu();
    item.action();
  }
</script>

{#each items as item}
  {#if item.sep}
    <div class="sep"></div>
  {:else if item.submenu}
    <div class="item has-sub">
      {item.label}
      <span class="arrow">»</span>
      <div class="menu submenu">
        <svelte:self items={item.submenu} />
      </div>
    </div>
  {:else}
    <div class="item" class:disabled={item.disabled} on:click={() => run(item)}>
      {#if item.checked !== undefined}
        <span class="check">{item.checked ? '✓' : ''}</span>
      {/if}
      {item.label}
    </div>
  {/if}
{/each}
