<script>
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import { fold } from './format.js';
  import { startsTypeahead } from './keys.js';

  export let label;
  export let nodes;
  export let selected;
  export let expanded;
  export let primary = false;
  export let onselect;
  export let ontoggle;
  export let onmenu;

  const TYPEAHEAD_RESET_MS = 700;

  let container;
  let typed = '';
  let typedAt = 0;

  $: at = nodes.findIndex(({ node }) => node.path === selected);

  function choose(index) {
    onselect(nodes[index].node);
    const row = container.children[index];
    if (row) row.scrollIntoView({ block: 'nearest' });
  }

  function parentIndex(index) {
    for (let i = index - 1; i >= 0; i--) {
      if (nodes[i].depth < nodes[index].depth) return i;
    }
    return index;
  }

  function typeTo(character) {
    const now = Date.now();
    typed = now - typedAt > TYPEAHEAD_RESET_MS ? character : typed + character;
    typedAt = now;
    const from = Math.max(0, at) + (typed.length === 1 ? 1 : 0);
    for (let offset = 0; offset < nodes.length; offset++) {
      const index = (from + offset) % nodes.length;
      if (fold(nodes[index].node.name).startsWith(fold(typed))) return choose(index);
    }
  }

  function keydown(event) {
    if (!nodes.length || event.ctrlKey || event.metaKey || event.altKey) return;
    const current = at === -1 ? null : nodes[at].node;
    if (event.key === 'ArrowDown') choose(Math.min(nodes.length - 1, at + 1));
    else if (event.key === 'ArrowUp') choose(Math.max(0, at - 1));
    else if (event.key === 'Home') choose(0);
    else if (event.key === 'End') choose(nodes.length - 1);
    else if (event.key === 'ArrowRight' && current) {
      if (!current.has_children) return;
      if (!expanded[current.path]) ontoggle(current);
      else if (nodes[at + 1]?.depth > nodes[at].depth) choose(at + 1);
    } else if (event.key === 'ArrowLeft' && current) {
      if (expanded[current.path]) ontoggle(current);
      else choose(parentIndex(at));
    } else if (event.key === 'Enter' && current) {
      if (current.has_children) ontoggle(current);
    } else if ((event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) && current) {
      const box = container.children[at].getBoundingClientRect();
      onmenu(current, { x: box.left + 24, y: box.bottom });
    } else if (
      event.key.length === 1 &&
      (Date.now() - typedAt <= TYPEAHEAD_RESET_MS || startsTypeahead(event))
    ) {
      event.stopPropagation();
      typeTo(event.key);
    } else return;
    event.preventDefault();
  }

  function press(event, node) {
    if (event.button !== 0) return;
    if (event.detail > 1) event.preventDefault();
    if (node.path !== selected) onselect(node);
  }

  function menu(event, node) {
    event.preventDefault();
    event.stopPropagation();
    if (node.path !== selected) onselect(node);
    container.focus();
    onmenu(node, { x: event.clientX, y: event.clientY });
  }
</script>

<div
  class="tree"
  role="tree"
  aria-label={label}
  tabindex="0"
  data-primary={primary ? '' : null}
  bind:this={container}
  on:keydown={keydown}
>
  {#each nodes as { node, depth } (node.path)}
    <div
      class="node"
      class:selected={selected === node.path}
      role="treeitem"
      aria-selected={selected === node.path}
      aria-expanded={node.has_children ? Boolean(expanded[node.path]) : null}
      aria-level={depth + 1}
      style:padding-left="{4 + depth * 14}px"
      on:mousedown={(e) => press(e, node)}
      on:dblclick={() => node.has_children && ontoggle(node)}
      on:contextmenu={(e) => menu(e, node)}
    >
      <span
        class="twisty"
        on:mousedown|stopPropagation={(e) => e.preventDefault()}
        on:click|stopPropagation={() => node.has_children && ontoggle(node)}
        >{#if node.has_children}{#if expanded[node.path]}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if}{/if}</span
      >
      <span class="name">{node.name}</span>
      {#if node.private}<span class="tag">private</span>{/if}
      {#if node.file_count}<span class="count">{node.file_count.toLocaleString()}</span>{/if}
    </div>
  {/each}
</div>
