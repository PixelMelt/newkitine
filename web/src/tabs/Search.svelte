<script>
  import { searches, wishlist, rooms, notice } from '../lib/stores.js';
  import { searchTarget, openBrowse, openUserInfo } from '../lib/ui.js';
  import { post, del } from '../lib/api.js';
  import { formatSize, formatAttributes, baseName, folderName } from '../lib/format.js';
  import { emptyFilters, compileFilters } from '../lib/filters.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  const MAX_ROWS = 1000;
  const RESULT_ACCESSORS = {
    username: (r) => r.response.username,
    speed: (r) => r.response.upload_speed,
    queue: (r) => r.response.queue_size,
    folder: (r) => folderName(r.file.name),
    filename: (r) => baseName(r.file.name),
    size: (r) => r.file.size,
    quality: (r) => formatAttributes(r.file.attributes),
  };

  let query = '';
  let wishTerm = '';
  let activeToken = null;
  let showWishlist = false;
  let mode = 'global';
  let modeRoom = '';
  let modeUser = '';
  let showFilters = false;
  let filters = emptyFilters();
  let resultsEl = null;
  let sort = { key: null, dir: 1 };

  async function focusResults() {
    for (let i = 0; i < 20 && !resultsEl; i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    resultsEl?.focus();
  }

  $: joinedRooms = Object.keys($rooms.joined).sort();
  $: if (mode === 'rooms' && !joinedRooms.includes(modeRoom)) modeRoom = joinedRooms[0] ?? '';
  $: activeSearch =
    $searches.find((s) => s.token === activeToken) ?? $searches[$searches.length - 1];
  $: rows = activeSearch
    ? activeSearch.results.flatMap((response) =>
        response.files.map((file) => ({ response, file })))
    : [];
  $: matches = compileFilters(filters);
  $: filteredRows = rows.filter(matches);
  $: shownRows = sortRows(filteredRows, sort, RESULT_ACCESSORS).slice(0, MAX_ROWS);

  $: if ($searchTarget) {
    query = $searchTarget;
    searchTarget.set(null);
    mode = 'global';
    startSearch();
  }

  async function startSearch() {
    if (!query.trim()) return;
    const body = { query: query.trim(), mode };
    if (mode === 'rooms') {
      if (!modeRoom) return;
      body.room = modeRoom;
    }
    if (mode === 'user') {
      if (!modeUser.trim()) return;
      body.user = modeUser.trim();
    }
    const { token } = await post('/searches', body);
    activeToken = token;
    query = '';
    focusResults();
  }

  function closeSearch(token) {
    del(`/searches/${token}`);
    if (activeToken === token) activeToken = null;
  }

  function download(username, file) {
    post('/downloads', {
      username,
      virtual_path: file.name,
      size: file.size,
      attributes: file.attributes,
    }).catch((error) =>
      notice(error.status === 409 ? 'download already active' : error.message),
    );
  }

  function rowMenu(event, response, file) {
    openMenu(event, [
      { label: 'Download File', action: () => download(response.username, file) },
      { sep: true },
      { label: 'Copy File Path', action: () => navigator.clipboard.writeText(file.name) },
      { sep: true },
      { label: 'View User Profile', action: () => openUserInfo(response.username) },
      {
        label: 'Browse Folder',
        action: () => openBrowse(response.username, folderName(file.name)),
      },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(response.username) },
    ]);
  }

  function tabMenu(event, search) {
    openMenu(event, [
      {
        label: 'Search Again',
        action: async () => {
          const { token } = await post('/searches', { query: search.query });
          activeToken = token;
        },
      },
      { label: 'Copy Search Term', action: () => navigator.clipboard.writeText(search.query) },
      { sep: true },
      { label: 'Close Tab', action: () => closeSearch(search.token) },
    ]);
  }

  function addWish() {
    if (!wishTerm.trim()) return;
    post('/wishlist', { term: wishTerm.trim() });
    wishTerm = '';
  }
</script>

<div class="toolbar">
  <input
    placeholder="Search files…"
    bind:value={query}
    on:keydown={(e) => e.key === 'Enter' && startSearch()}
  />
  <select bind:value={mode}>
    <option value="global">Global</option>
    <option value="rooms">Rooms</option>
    <option value="buddies">Buddies</option>
    <option value="user">User</option>
  </select>
  {#if mode === 'rooms'}
    <select bind:value={modeRoom}>
      {#each joinedRooms as room}
        <option value={room}>{room}</option>
      {/each}
    </select>
  {:else if mode === 'user'}
    <input style="min-width: 120px;" placeholder="Username…" bind:value={modeUser} />
  {/if}
  <button on:click={startSearch}>Search</button>
  <button class:active={showFilters} on:click={() => (showFilters = !showFilters)}>
    Result Filters
  </button>
  <button on:click={() => (showWishlist = !showWishlist)}>Wishlist ({$wishlist.length})</button>
</div>

{#if showFilters}
  <div class="toolbar">
    <input style="min-width: 120px;" placeholder="Include text…" bind:value={filters.include} />
    <input style="min-width: 120px;" placeholder="Exclude text…" bind:value={filters.exclude} />
    <input
      style="min-width: 100px;"
      placeholder="File type, e.g. flac !mp3"
      bind:value={filters.type}
    />
    <input
      style="min-width: 100px;"
      placeholder="Size, e.g. >10.5m <1g"
      bind:value={filters.size}
    />
    <input
      style="min-width: 90px;"
      placeholder="Bitrate, e.g. 320 <1412"
      bind:value={filters.bitrate}
    />
    <input
      style="min-width: 90px;"
      placeholder="Duration, e.g. >6:00"
      bind:value={filters.duration}
    />
    <label><input type="checkbox" bind:checked={filters.freeSlot} /> Free slot</label>
    <button on:click={() => (filters = emptyFilters())}>Clear Filters</button>
  </div>
{/if}

{#if showWishlist}
  <div class="toolbar">
    <input
      placeholder="Add wish…"
      bind:value={wishTerm}
      on:keydown={(e) => e.key === 'Enter' && addWish()}
    />
    <button on:click={addWish}>Add</button>
    {#each $wishlist as term}
      <span>
        {term}
        <button on:click={() => post('/wishlist/remove', { term })}>Remove</button>
      </span>
    {/each}
  </div>
{/if}

{#if $searches.length}
  <div class="toolbar">
    {#each $searches as search}
      <button
        class:active={activeSearch?.token === search.token}
        on:click={() => ((activeToken = search.token), focusResults())}
        on:contextmenu={(e) => tabMenu(e, search)}
      >
        {search.query}
        ({search.results.reduce((n, r) => n + r.files.length, 0)})
      </button>
    {/each}
    {#if activeSearch}
      <button on:click={() => closeSearch(activeSearch.token)}>Close</button>
    {/if}
  </div>
{/if}

{#if activeSearch}
  {#if filteredRows.length !== rows.length || rows.length > MAX_ROWS}
    <span>showing {shownRows.length} of {rows.length} results</span>
  {/if}
  <div class="scroll" tabindex="0" bind:this={resultsEl}>
    <table>
      <thead>
        <tr>
          <Th bind:sort key="username">User</Th>
          <Th bind:sort key="speed">Speed</Th>
          <Th bind:sort key="queue">Queue</Th>
          <Th bind:sort key="folder" grow>Folder</Th>
          <Th bind:sort key="filename" grow>Filename</Th>
          <Th bind:sort key="size">Size</Th>
          <Th bind:sort key="quality">Quality</Th>
        </tr>
      </thead>
      <tbody>
        {#each shownRows as { response, file }}
          <tr
            class="clickable"
            on:contextmenu={(e) => rowMenu(e, response, file)}
            on:dblclick={() => download(response.username, file)}
          >
            <td>{response.username}{response.free_upload_slots ? '' : ' (queued)'}</td>
            <td>{formatSize(response.upload_speed)}/s</td>
            <td>{response.queue_size}</td>
            <td class="grow">{folderName(file.name)}</td>
            <td class="grow">{baseName(file.name)}</td>
            <td>{formatSize(file.size)}</td>
            <td>{formatAttributes(file.attributes)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
