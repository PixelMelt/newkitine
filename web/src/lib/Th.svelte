<script>
  export let sort;
  export let key;
  export let grow = false;

  function toggle() {
    sort = sort.key === key ? { key, dir: -sort.dir } : { key, dir: 1 };
  }
</script>

<th class="sortable" class:grow on:click={toggle}>
  <slot />{#if sort.key === key}<span class="dir">{sort.dir === 1 ? '▲' : '▼'}</span>{/if}
</th>

<style>
  th.sortable {
    cursor: pointer;
    user-select: none;
  }
  th.sortable:hover {
    background: var(--hover-strong);
  }
  .dir {
    margin-left: 4px;
    color: var(--text-dim);
    font-size: 10px;
  }
</style>
