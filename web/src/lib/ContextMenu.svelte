<script>
  import { menu, closeMenu } from './menu.js';
  import MenuList from './MenuList.svelte';

  let width = 0;
  let height = 0;

  $: x = $menu ? Math.max(0, Math.min($menu.x, window.innerWidth - width - 4)) : 0;
  $: y = $menu ? Math.max(0, Math.min($menu.y, window.innerHeight - height - 4)) : 0;
</script>

<svelte:window
  on:click={closeMenu}
  on:contextmenu={closeMenu}
  on:keydown={(e) => e.key === 'Escape' && closeMenu()}
/>

{#if $menu}
  <div
    class="menu"
    bind:offsetWidth={width}
    bind:offsetHeight={height}
    style:left="{x}px"
    style:top="{y}px"
  >
    <MenuList items={$menu.items} />
  </div>
{/if}
