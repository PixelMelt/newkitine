<script>
  import { tick } from 'svelte';
  import {
    searches, wishlist, rooms, downloadIndex, transferKey, queueDownloads, startSearch, scopeOf,
    report,
  } from '../lib/stores.js';
  import { route, go, remember, forget, openBrowse, noAutofill } from '../lib/ui.js';
  import { post, del } from '../lib/api.js';
  import { act, copy } from '../lib/act.js';
  import {
    formatSize, formatSpeed, formatQuality, formatDuration, formatCount, effectiveBitrate, plural,
  } from '../lib/format.js';
  import { emptyFilters, compileFilters, activeFilterCount } from '../lib/filters.js';
  import { shortState } from '../lib/transferStatus.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenu, openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import X from '@lucide/svelte/icons/x';
  import Subtabs from '../lib/Subtabs.svelte';
  import DataTable from '../lib/DataTable.svelte';

  let query = '';
  let wishTerm = '';
  let showWishlist = false;
  let mode = 'global';
  let modeRoom = '';
  let modeUser = '';
  let showFilters = false;
  let filters = emptyFilters();
  let queryInput;
  let table;
  let strip;
  let selection = new Set();
  let started = null;

  const stateOf = (index, row) => index.get(transferKey(row.response.username, row.file.name));

  const makeColumns = (index) => [
    { key: 'username', label: 'User', width: 140, text: (r) => r.response.username },
    {
      key: 'speed', label: 'Speed', width: 88, align: 'end', desc: true,
      text: (r) => formatSpeed(r.response.upload_speed), sortValue: (r) => r.response.upload_speed,
    },
    {
      key: 'queue', label: 'Queue', width: 64, align: 'end',
      text: (r) => (r.response.free_upload_slots ? 'Free' : formatCount(r.response.queue_size)),
      sortValue: (r) => (r.response.free_upload_slots ? -1 : r.response.queue_size),
      class: (r) => (r.response.free_upload_slots ? '' : 'dim'),
    },
    { key: 'folder', label: 'Folder', grow: 2, min: 160, tail: 20, text: (r) => r.folder },
    { key: 'filename', label: 'Filename', grow: 3, min: 200, tail: 12, text: (r) => r.filename },
    {
      key: 'size', label: 'Size', width: 84, align: 'end', desc: true,
      text: (r) => formatSize(r.file.size), sortValue: (r) => r.file.size,
    },
    {
      key: 'quality', label: 'Quality', width: 124, desc: true,
      text: (r) => formatQuality(r.file.attributes),
      sortValue: (r) => effectiveBitrate(r.file.attributes),
    },
    {
      key: 'length', label: 'Length', width: 64, align: 'end', desc: true,
      text: (r) => (r.file.attributes.length ? formatDuration(r.file.attributes.length) : ''),
      sortValue: (r) => r.file.attributes.length ?? 0,
    },
    {
      key: 'state', label: 'Download', width: 80,
      text: (r) => (stateOf(index, r) ? shortState(stateOf(index, r)) : ''),
      class: (r) => (stateOf(index, r) && shortState(stateOf(index, r)) === 'Failed' ? 'bad' : 'dim'),
    },
  ];

  $: columns = makeColumns($downloadIndex);
  $: joinedRooms = [...$rooms.joined.keys()].sort();
  $: if (mode === 'rooms' && !joinedRooms.includes(modeRoom)) modeRoom = joinedRooms[0] ?? '';
  $: wanted = $route.items.search ?? null;
  $: activeSearch = $searches.find((s) => String(s.token) === wanted) ?? null;
  $: if (wanted === null && $searches.length) {
    const newest = String($searches.at(-1).token);
    tick().then(() => remember('search', newest));
  }
  $: rows = activeSearch ? activeSearch.rows : [];
  $: compiled = compileFilters(filters);
  $: filterCount = activeFilterCount(filters);
  $: filteredRows = filterCount ? rows.filter(compiled.matches) : rows;
  $: selected = filteredRows.filter((r) => selection.has(r.key));
  $: items = $searches.map((search) => ({
    id: String(search.token),
    label: search.query,
    count: formatCount(search.rows.length),
  }));

  function scope() {
    const body = { query: query.trim(), mode };
    if (mode === 'rooms') body.room = modeRoom;
    if (mode === 'user') body.user = modeUser.trim();
    return body;
  }

  async function submit() {
    const body = scope();
    if (!body.query) return;
    if (mode === 'rooms' && !body.room) {
      report.error('Search not started: join a room first, or search globally.');
      return;
    }
    if (mode === 'user' && !body.user) {
      report.error('Search not started: enter the user whose files to search.');
      return;
    }
    const signature = JSON.stringify(body);
    if (started?.signature === signature && $searches.some((s) => s.token === started.token)) {
      go('search', String(started.token));
      return;
    }
    if (await startSearch(body)) {
      started = { signature, token: Number($route.items.search) };
      queryInput.select();
    }
  }

  function closeSearch(id) {
    const search = $searches.find((s) => String(s.token) === id);
    const scope = scopeOf(search);
    const next = strip.neighbour(id);
    if (wanted === id) {
      if (next) go('search', next);
      else forget('search', id);
    }
    act(() => del(`/searches/${id}`), {
      failed: `Search "${search.query}" not closed`,
      done: `Closed search "${search.query}" (${plural(search.rows.length, 'result')})`,
      action: { label: 'Search Again', run: () => startSearch(scope) },
    });
  }

  const download = (picked) =>
    queueDownloads(
      picked.map(({ response, file }) => ({
        username: response.username,
        virtual_path: file.name,
        size: file.size,
        attributes: file.attributes,
      })),
    );

  function downloadFolders(picked) {
    const folders = new Map(picked.map((r) => [`${r.response.username}\u0000${r.folder}`, r]));
    for (const { response, folder } of folders.values()) {
      act(() => post('/downloads/folder/request', { username: response.username, dir: folder }), {
        failed: `Folder "${folder}" not requested`,
        done: `Asked ${response.username} for the contents of "${folder}"`,
        action: { label: 'Show Downloads', run: () => go('downloads') },
      });
    }
  }

  function rowMenu(picked, point) {
    const many = picked.length > 1;
    const first = picked[0];
    const sameUser = picked.every((r) => r.response.username === first.response.username);
    openMenuAt(point, [
      {
        label: many ? `Download ${picked.length} Files` : 'Download File',
        primary: true, action: () => download(picked),
      },
      { label: many ? 'Download Folders' : 'Download Folder', action: () => downloadFolders(picked) },
      { sep: true },
      {
        label: 'Browse Folder', disabled: many,
        action: () => openBrowse(first.response.username, first.folder),
      },
      {
        label: many ? `Copy ${picked.length} File Paths` : 'Copy File Path',
        action: () => copy(picked.map((r) => r.file.name).join('\n'), many ? 'file paths' : 'file path'),
      },
      { sep: true },
      { label: 'User Actions', disabled: !sameUser, submenu: userMenu(first.response.username) },
    ]);
  }

  function tabMenu(event, id) {
    const search = $searches.find((s) => String(s.token) === id);
    openMenu(event, [
      { label: 'Search Again', action: () => startSearch(scopeOf(search)) },
      { label: 'Copy Search Term', action: () => copy(search.query, 'search term') },
      { sep: true },
      { label: 'Close Search', action: () => closeSearch(id) },
    ]);
  }

  async function addWish() {
    const term = wishTerm.trim();
    if (!term) return;
    if (await act(() => post('/wishlist', { term }), { failed: `"${term}" not added to wishlist` })) {
      wishTerm = '';
    }
  }

  function removeWish(term) {
    act(() => post('/wishlist/remove', { term }), {
      failed: `"${term}" not removed from wishlist`,
      done: `Removed "${term}" from wishlist`,
      action: {
        label: 'Undo',
        run: () => act(() => post('/wishlist', { term }), { failed: `"${term}" not restored` }),
      },
    });
  }

  async function queryKey(event) {
    if (event.key !== 'ArrowDown' || !activeSearch) return;
    event.preventDefault();
    await tick();
    table.focusFirst();
  }

  export function focusQuery() {
    queryInput.focus();
    queryInput.select();
  }
</script>

<div class="toolbar">
  <input {...noAutofill}
    class="query"
    data-primary={activeSearch ? null : ''}
    data-search-query
    placeholder="Search files…"
    bind:value={query}
    bind:this={queryInput}
    on:keydown={onEnter(submit)}
    on:keydown={queryKey}
  />
  <select bind:value={mode} aria-label="Search scope">
    <option value="global">Everyone</option>
    <option value="rooms">A room</option>
    <option value="buddies">Buddies</option>
    <option value="user">One user</option>
  </select>
  {#if mode === 'rooms'}
    <select bind:value={modeRoom} aria-label="Room to search">
      {#each joinedRooms as room}
        <option value={room}>{room}</option>
      {:else}
        <option value="" disabled>No rooms joined</option>
      {/each}
    </select>
  {:else if mode === 'user'}
    <input {...noAutofill}
      class="narrow"
      placeholder="Shared by…"
      bind:value={modeUser}
      on:keydown={onEnter(submit)}
    />
  {/if}
  <button on:click={submit}>Search</button>
  <button
    class:active={showFilters || filterCount > 0}
    aria-pressed={showFilters}
    on:click={() => (showFilters = !showFilters)}
  >
    Result Filters{filterCount ? ` (${filterCount})` : ''}
  </button>
  <button
    class:active={showWishlist}
    aria-pressed={showWishlist}
    on:click={() => (showWishlist = !showWishlist)}
  >
    Wishlist ({$wishlist.length})
  </button>
</div>

{#if showFilters}
  <div class="toolbar filters">
    <label>Include <input {...noAutofill} class="narrow" placeholder="live | remaster" bind:value={filters.include} /></label>
    <label>Exclude <input {...noAutofill} class="narrow" placeholder="karaoke | cover" bind:value={filters.exclude} /></label>
    <label>Type <input {...noAutofill} class="tiny" placeholder="flac !mp3" bind:value={filters.type} /></label>
    <label>
      Size
      <input {...noAutofill} class="tiny" class:invalid={compiled.invalid.size}
        placeholder="10m <1g" title="Minimum size, or <maximum. Units: k, m, g." bind:value={filters.size} />
    </label>
    <label>
      Bitrate
      <input {...noAutofill} class="tiny" class:invalid={compiled.invalid.bitrate}
        placeholder="320 <1412" title="Minimum kbps, or <maximum, or =exact." bind:value={filters.bitrate} />
    </label>
    <label>
      Length
      <input {...noAutofill} class="tiny" class:invalid={compiled.invalid.duration}
        placeholder="2:00 <10:00" title="Minimum length as m:ss, or <maximum." bind:value={filters.duration} />
    </label>
    <label><input type="checkbox" bind:checked={filters.freeSlot} /> Free slot only</label>
    <button on:click={() => (filters = emptyFilters())}>Clear Filters</button>
    {#if Object.values(compiled.invalid).some(Boolean)}
      <span class="notice">The outlined field has a part that is not a number and is being ignored.</span>
    {/if}
  </div>
{/if}

{#if showWishlist}
  <div class="toolbar">
    <input {...noAutofill}
      placeholder="Add wish…"
      bind:value={wishTerm}
      on:keydown={onEnter(addWish)}
    />
    <button on:click={addWish}>Add</button>
    {#each $wishlist as term}
      <span class="chip">
        {term}
        <button title="Remove from wishlist" aria-label="Remove {term}" on:click={() => removeWish(term)}><X size={12} /></button>
      </span>
    {/each}
  </div>
{/if}

<Subtabs
  tab="search"
  {items}
  active={wanted}
  bind:this={strip}
  onselect={(id) => go('search', id)}
  onclose={closeSearch}
  onmenu={tabMenu}
/>

{#if activeSearch}
  {#if selected.length > 1 || filteredRows.length !== rows.length}
  <div class="count">
    {#if selected.length > 1}
      {formatCount(selected.length)} of {plural(filteredRows.length, 'result')} selected ·
      {formatSize(selected.reduce((sum, r) => sum + r.file.size, 0))}
    {:else if filteredRows.length !== rows.length}
      {formatCount(filteredRows.length)} of {plural(rows.length, 'result')} ·
      {formatCount(rows.length - filteredRows.length)} hidden by filters
      <button class="link" on:click={() => (filters = emptyFilters())}>Clear Filters</button>
    {/if}
  </div>
  {/if}
  <DataTable
    id="search"
    label="Search results"
    rows={filteredRows}
    key={(r) => r.key}
    {columns}
    viewKey={wanted}
    primary
    bind:selection
    bind:this={table}
    empty={rows.length
      ? 'No results match the filters.'
      : 'No results yet.'}
    copyText={(r) => r.file.name}
    onactivate={download}
    onmenu={rowMenu}
    onedge={focusQuery}
  />
{/if}
