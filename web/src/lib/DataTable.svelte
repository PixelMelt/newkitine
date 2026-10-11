<script>
  import { tick } from 'svelte';
  import { sortRows } from './sort.js';
  import { fold } from './format.js';
  import { startsTypeahead } from './keys.js';
  import { copy } from './act.js';

  export let id;
  export let label;
  export let rows;
  export let key;
  export let columns;
  export let viewKey = null;
  export let primary = false;
  export let empty = '';
  export let loading = false;
  export let selection = new Set();
  export let typeahead = null;
  export let copyText = null;
  export let onpick = null;
  export let onactivate = null;
  export let onmenu = null;
  export let ondelete = null;
  export let onedge = null;

  const ROW = 22;
  const OVERSCAN = 8;
  const TYPEAHEAD_RESET_MS = 700;
  const sortStorageKey = `table.${id}.sort`;

  let container;
  let scrollTop = 0;
  let height = 0;
  let cursor = null;
  let anchor = null;
  let sort = loadSort();
  let currentView = viewKey;
  let typed = '';
  let typedAt = 0;
  const scrollByView = new Map();

  function loadSort() {
    const stored = JSON.parse(localStorage.getItem(sortStorageKey));
    return stored && columns.some((c) => c.key === stored.key) ? stored : null;
  }

  $: template = columns
    .map((c) => (c.width ? `${c.width}px` : `minmax(${c.min}px, ${c.grow}fr)`))
    .join(' ');
  $: minWidth = columns.reduce((sum, c) => sum + (c.width ?? c.min), 0);
  $: sorted = resort(rows, sort);
  $: if (height) scrollTop = container.scrollTop;
  $: indexOf = new Map(sorted.map((row, index) => [key(row), index]));
  $: if ([...selection].some((k) => !indexOf.has(k))) {
    selection = new Set([...selection].filter((k) => indexOf.has(k)));
  }
  $: start = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  $: end = Math.min(sorted.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);
  $: visible = sorted.slice(start, end);
  $: if (viewKey !== currentView) switchView(viewKey);

  function resort(list, order) {
    return sortRows(list, order, columns);
  }

  async function switchView(next) {
    scrollByView.set(currentView, scrollTop);
    currentView = next;
    selection = new Set();
    cursor = null;
    anchor = null;
    await tick();
    container.scrollTop = scrollByView.get(next) ?? 0;
  }

  export function focusFirst() {
    container.focus();
    if (sorted.length && !selection.size) moveTo(0, false);
  }

  function selectedRows() {
    return sorted.filter((row) => selection.has(key(row)));
  }

  function cycleSort(column) {
    const first = column.desc ? -1 : 1;
    if (sort?.key !== column.key) sort = { key: column.key, dir: first };
    else if (sort.dir === first) sort = { key: column.key, dir: -first };
    else sort = null;
    localStorage.setItem(sortStorageKey, JSON.stringify(sort));
  }

  function range(from, to) {
    const [low, high] = from < to ? [from, to] : [to, from];
    return sorted.slice(low, high + 1).map(key);
  }

  function pick(event, k) {
    const index = indexOf.get(k);
    const additive = event.ctrlKey || event.metaKey;
    if (event.shiftKey && indexOf.has(anchor)) {
      const span = range(indexOf.get(anchor), index);
      selection = new Set(additive ? [...selection, ...span] : span);
    } else if (additive) {
      const next = new Set(selection);
      if (!next.delete(k)) next.add(k);
      selection = next;
      anchor = k;
    } else {
      selection = new Set([k]);
      anchor = k;
      if (onpick) onpick(sorted[index]);
    }
    cursor = k;
  }

  function moveTo(index, extend) {
    const k = key(sorted[index]);
    if (extend && indexOf.has(anchor)) {
      selection = new Set(range(indexOf.get(anchor), index));
    } else {
      selection = new Set([k]);
      anchor = k;
      if (onpick) onpick(sorted[index]);
    }
    cursor = k;
    reveal(index);
  }

  function reveal(index) {
    const top = index * ROW;
    const viewport = height - ROW;
    if (top < container.scrollTop) container.scrollTop = top;
    else if (top + ROW > container.scrollTop + viewport) container.scrollTop = top + ROW - viewport;
  }

  function rowDown(event, row) {
    if (event.button !== 0) return;
    if (event.detail > 1) event.preventDefault();
    pick(event, key(row));
  }

  function rowMenu(event, row) {
    event.preventDefault();
    event.stopPropagation();
    const k = key(row);
    if (!selection.has(k)) {
      selection = new Set([k]);
      anchor = k;
      cursor = k;
    }
    container.focus();
    onmenu(selectedRows(), { x: event.clientX, y: event.clientY });
  }

  function backgroundDown(event) {
    if (event.target.closest('.row, .head')) return;
    const box = container.getBoundingClientRect();
    if (event.clientX - box.left >= container.clientWidth) return;
    if (event.clientY - box.top >= container.clientHeight) return;
    selection = new Set();
  }

  function keyMenu() {
    const index = selection.has(cursor)
      ? indexOf.get(cursor)
      : sorted.findIndex((row) => selection.has(key(row)));
    reveal(index);
    const box = container.getBoundingClientRect();
    onmenu(selectedRows(), {
      x: box.left + 24,
      y: box.top + ROW + (index + 1) * ROW - container.scrollTop,
    });
  }

  function typeTo(character) {
    const now = Date.now();
    typed = now - typedAt > TYPEAHEAD_RESET_MS ? character : typed + character;
    typedAt = now;
    const wanted = fold(typed);
    const from = indexOf.has(cursor) ? indexOf.get(cursor) + (typed.length === 1 ? 1 : 0) : 0;
    for (let offset = 0; offset < sorted.length; offset++) {
      const index = (from + offset) % sorted.length;
      if (fold(typeahead(sorted[index])).startsWith(wanted)) {
        moveTo(index, false);
        return;
      }
    }
  }

  function keydown(event) {
    const command = event.ctrlKey || event.metaKey;
    const count = sorted.length;
    const at = indexOf.has(cursor) ? indexOf.get(cursor) : -1;
    const page = Math.max(1, Math.floor((height - ROW) / ROW) - 1);
    const targets = {
      ArrowDown: Math.min(count - 1, at + 1),
      ArrowUp: Math.max(0, at - 1),
      Home: 0,
      End: count - 1,
      PageDown: Math.min(count - 1, at + page),
      PageUp: Math.max(0, at - page),
    };
    if (event.key in targets && !command && !event.altKey) {
      event.preventDefault();
      if (event.key === 'ArrowUp' && at <= 0 && onedge) onedge('top');
      else if (count) moveTo(targets[event.key], event.shiftKey);
    } else if (event.key === 'Enter' && selection.size && onactivate) {
      event.preventDefault();
      onactivate(selectedRows());
    } else if (command && event.key.toLowerCase() === 'a') {
      event.preventDefault();
      selection = new Set(sorted.map(key));
    } else if (command && event.key.toLowerCase() === 'c' && selection.size && copyText) {
      event.preventDefault();
      copy(selectedRows().map(copyText).join('\n'), selection.size > 1 ? 'selected rows' : 'selected row');
    } else if ((event.key === 'Delete' || event.key === 'Backspace') && selection.size && ondelete) {
      event.preventDefault();
      ondelete(selectedRows());
    } else if (
      (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) &&
      selection.size &&
      onmenu
    ) {
      event.preventDefault();
      keyMenu();
    } else if (event.key === 'Escape' && selection.size) {
      selection = new Set();
    } else if (typeahead && event.key.length === 1 && !command && !event.altKey && count) {
      const continuing = Date.now() - typedAt <= TYPEAHEAD_RESET_MS;
      if (!continuing && !startsTypeahead(event)) return;
      event.preventDefault();
      event.stopPropagation();
      typeTo(event.key);
    }
  }

  function hint(event) {
    const cell = event.target.closest('.cell');
    if (!cell) return;
    const text = cell.firstElementChild;
    const parts = [];
    if (text.scrollWidth > text.clientWidth) parts.push(cell.textContent);
    if (cell.dataset.hint) parts.push(cell.dataset.hint);
    if (parts.length) cell.title = parts.join('\n');
    else cell.removeAttribute('title');
  }

  function split(text, tail) {
    return text.length > tail * 2 ? [text.slice(0, -tail), text.slice(-tail)] : [text, ''];
  }
</script>

<div
  class="datatable"
  role="grid"
  aria-label={label}
  aria-multiselectable="true"
  aria-rowcount={sorted.length}
  tabindex="0"
  data-primary={primary ? '' : null}
  bind:this={container}
  bind:clientHeight={height}
  on:scroll={() => (scrollTop = container.scrollTop)}
  on:keydown={keydown}
  on:mousedown={backgroundDown}
  on:mouseover={hint}
>
  <div class="head" role="row" style:grid-template-columns={template} style:min-width="{minWidth}px">
    {#each columns as column}
      <div
        class="cell"
        class:end={column.align === 'end'}
        role="columnheader"
        aria-sort={sort?.key === column.key ? (sort.dir === 1 ? 'ascending' : 'descending') : null}
        on:click={() => cycleSort(column)}
      >
        <span class="text">{column.label}</span>{#if sort?.key === column.key}<span class="dir"
            >{sort.dir === 1 ? '▲' : '▼'}</span
          >{/if}
      </div>
    {/each}
  </div>
  {#if sorted.length}
    <div class="body" style:height="{sorted.length * ROW}px" style:min-width="{minWidth}px">
      <div class="window" style:transform="translateY({start * ROW}px)">
        {#each visible as row (key(row))}
          <div
            class="row"
            class:selected={selection.has(key(row))}
            class:cursor={cursor === key(row)}
            role="row"
            aria-selected={selection.has(key(row))}
            style:grid-template-columns={template}
            on:mousedown={(e) => rowDown(e, row)}
            on:dblclick={() => onactivate && onactivate(selectedRows())}
            on:contextmenu={(e) => onmenu && rowMenu(e, row)}
          >
            {#each columns as column}
              <div
                class="cell {column.class ? column.class(row) : ''}"
                class:end={column.align === 'end'}
                role="gridcell"
                data-hint={column.hint ? column.hint(row) : null}
              >
                {#if column.progress}
                  {@const fraction = column.progress(row)}
                  <span class="bar"><span style:width="{fraction * 100}%"></span></span>
                  <span class="percent">{Math.floor(fraction * 100)}%</span>
                {:else if column.tail}
                  {@const [lead, tail] = split(column.text(row), column.tail)}
                  <span class="text">{lead}</span><span class="tail">{tail}</span>
                {:else}
                  <span class="text">{column.text(row)}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>
    </div>
  {:else}
    <div class="note">{loading ? 'Loading…' : empty}</div>
  {/if}
</div>

<style>
  .datatable {
    position: relative;
    overflow: auto;
    flex: 1;
    min-height: 0;
    border: 1px solid var(--border-soft);
    background: var(--panel);
    font-variant-numeric: tabular-nums;
    user-select: none;
    cursor: default;
  }
  .head,
  .row {
    display: grid;
    height: 22px;
    line-height: 21px;
  }
  .head {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--chrome);
    font-weight: 600;
    border-bottom: 1px solid var(--border-faint);
  }
  .head .cell:hover {
    background: var(--hover-strong);
  }
  .body {
    position: relative;
    overflow: hidden;
  }
  .row {
    border-bottom: 1px solid var(--border-faint);
  }
  .cell {
    display: flex;
    min-width: 0;
    padding: 0 6px;
    border-right: 1px solid var(--border-faint);
    white-space: pre;
  }
  .cell.end {
    justify-content: flex-end;
  }
  .text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tail {
    flex-shrink: 0;
  }
  .dir {
    flex-shrink: 0;
    margin-left: 4px;
    color: var(--text-dim);
    font-size: 10px;
  }
  .row.selected {
    background: var(--selected-idle);
  }
  .datatable:focus .row.selected {
    background: var(--selected);
    color: var(--selected-text);
  }
  .datatable:focus .row.cursor {
    outline: 1px dotted var(--text-dim);
    outline-offset: -1px;
  }
  .cell.dim {
    color: var(--text-dim);
  }
  .cell.bad {
    color: var(--danger);
  }
  .datatable:focus .row.selected .cell.dim,
  .datatable:focus .row.selected .cell.bad {
    color: inherit;
  }
  .bar {
    align-self: center;
    flex: 1;
    min-width: 0;
    height: 8px;
    margin-right: 6px;
    background: var(--chrome);
    border: 1px solid var(--border-soft);
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .percent {
    flex-shrink: 0;
    width: 4ch;
    text-align: right;
  }
  .note {
    padding: 6px 8px;
    color: var(--text-dim);
  }
</style>
