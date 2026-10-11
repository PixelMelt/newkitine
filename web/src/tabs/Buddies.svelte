<script>
  import { tick } from 'svelte';
  import { buddies, banned, ignored } from '../lib/stores.js';
  import { route, openUserInfo, noAutofill } from '../lib/ui.js';
  import { get, post } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { formatSpeed, formatCount, plural } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu, addBuddy, removeBuddy, ban, unban, ignore, unignore } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';

  const STATUS = {
    online: ['Online', 0], away: ['Away', 1], offline: ['Offline', 2], unknown: ['Unknown', 3],
  };

  const columns = [
    { key: 'username', label: 'User', grow: 1, min: 140, text: (b) => b.username },
    {
      key: 'status', label: 'Status', width: 80, text: (b) => STATUS[b.status][0],
      sortValue: (b) => STATUS[b.status][1], class: (b) => (b.status === 'online' || b.status === 'away' ? '' : 'dim'),
    },
    {
      key: 'speed', label: 'Speed', width: 96, align: 'end', desc: true,
      text: (b) => formatSpeed(b.stats.avgspeed), sortValue: (b) => b.stats.avgspeed,
    },
    {
      key: 'files', label: 'Files', width: 84, align: 'end', desc: true,
      text: (b) => formatCount(b.stats.files), sortValue: (b) => b.stats.files,
    },
    { key: 'privileged', label: 'Privileged', width: 84, text: (b) => (b.privileged ? 'Yes' : '') },
    { key: 'note', label: 'Note', grow: 2, min: 140, text: (b) => b.note },
  ];
  const nameColumn = (label) => [{ key: 'name', label, grow: 1, min: 100, text: (name) => name }];

  let buddyName = '';
  let banName = '';
  let ignoreName = '';
  let ipPattern = '';
  let ipBans = [];
  let ipError = '';
  let editing = null;
  let noteText = '';
  let noteInput;

  const byName = (a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' });

  $: list = [...$buddies.values()].sort((a, b) => byName(a.username, b.username));
  $: banRows = [...$banned].sort(byName);
  $: ignoreRows = [...$ignored].sort(byName);
  $: if ($route.tab === 'buddies') loadIpBans();

  async function loadIpBans() {
    ipBans = (await get('/ip_bans')).patterns;
  }

  async function submit(run, name, clear) {
    const username = name.trim();
    if (username && (await run(username))) clear();
  }

  async function editNote(buddy) {
    editing = buddy.username;
    noteText = buddy.note;
    await tick();
    noteInput.focus();
    noteInput.select();
  }

  async function saveNote() {
    const username = editing;
    const saved = await act(
      () => post(`/buddies/${encodeURIComponent(username)}/note`, { note: noteText }),
      { failed: `Note for ${username} not saved` },
    );
    if (saved) editing = null;
  }

  function buddyMenu(rows, point) {
    const buddy = rows[0];
    openMenuAt(point, [
      ...userMenu(buddy.username, { primary: true }),
      { sep: true },
      { label: buddy.note ? 'Edit Note' : 'Add Note', action: () => editNote(buddy) },
    ]);
  }

  async function addIpBan() {
    const pattern = ipPattern.trim();
    if (!pattern) return;
    ipError = '';
    const added = await act(() => post('/ip_bans', { pattern }), {
      failed: 'IP ban not added',
      on: { 400: 'use four dot-separated numbers, with * as a wildcard, like 10.0.*.*' },
      where: (text) => (ipError = text),
      done: `Banned IP address ${pattern}`,
      action: { label: 'Undo', run: () => removeIpBans([pattern]) },
    });
    if (added) {
      ipPattern = '';
      loadIpBans();
    }
  }

  async function removeIpBans(patterns) {
    await act(() => Promise.all(patterns.map((pattern) => post('/ip_bans/remove', { pattern }))), {
      failed: 'IP ban not removed',
      done: `Removed ${plural(patterns.length, 'IP ban')}`,
    });
    loadIpBans();
  }

  const removal = (label, run, withUser) => (rows, point) =>
    openMenuAt(point, [
      ...(withUser ? [...userMenu(rows[0], { primary: true }), { sep: true }] : []),
      { label, action: () => run(rows) },
    ]);
</script>

<div class="toolbar">
  <input {...noAutofill}
    placeholder="Add buddy…"
    bind:value={buddyName}
    on:keydown={onEnter(() => submit(addBuddy, buddyName, () => (buddyName = '')))}
  />
  <button on:click={() => submit(addBuddy, buddyName, () => (buddyName = ''))}>Add Buddy</button>
  <span class="count">{plural(list.length, 'buddy', 'buddies')}</span>
</div>

{#if editing}
  <div class="toolbar">
    <label for="buddy-note">Note for {editing}</label>
    <input {...noAutofill}
      id="buddy-note"
      class="fill"
      bind:value={noteText}
      bind:this={noteInput}
      on:keydown={onEnter(saveNote)}
      on:keydown={(e) => e.key === 'Escape' && (editing = null)}
    />
    <button on:click={saveNote}>Save Note</button>
    <button on:click={() => (editing = null)}>Cancel</button>
  </div>
{/if}

<DataTable
  id="buddies"
  label="Buddies"
  rows={list}
  key={(b) => b.username}
  {columns}
  primary
  typeahead={(b) => b.username}
  copyText={(b) => b.username}
  empty="No buddies."
  onactivate={(rows) => openUserInfo(rows[0].username)}
  onmenu={buddyMenu}
  ondelete={(rows) => rows.forEach((b) => removeBuddy(b.username))}
/>

<div class="split short">
  <div class="main">
    <h3>Banned Users</h3>
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        placeholder="Name to ban…"
        bind:value={banName}
        on:keydown={onEnter(() => submit(ban, banName, () => (banName = '')))}
      />
      <button on:click={() => submit(ban, banName, () => (banName = ''))}>Ban</button>
    </div>
    <DataTable
      id="banned"
      label="Banned users"
      rows={banRows}
      key={(name) => name}
      columns={nameColumn('User')}
      headless
      typeahead={(name) => name}
      copyText={(name) => name}
      empty="Nobody is banned."
      onactivate={(rows) => openUserInfo(rows[0])}
      onmenu={removal('Unban', (rows) => rows.forEach(unban), true)}
      ondelete={(rows) => rows.forEach(unban)}
    />
  </div>
  <div class="main">
    <h3>Ignored Users</h3>
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        placeholder="Name to ignore…"
        bind:value={ignoreName}
        on:keydown={onEnter(() => submit(ignore, ignoreName, () => (ignoreName = '')))}
      />
      <button on:click={() => submit(ignore, ignoreName, () => (ignoreName = ''))}>Ignore</button>
    </div>
    <DataTable
      id="ignored"
      label="Ignored users"
      rows={ignoreRows}
      key={(name) => name}
      columns={nameColumn('User')}
      headless
      typeahead={(name) => name}
      copyText={(name) => name}
      empty="Nobody is ignored."
      onactivate={(rows) => openUserInfo(rows[0])}
      onmenu={removal('Stop Ignoring', (rows) => rows.forEach(unignore), true)}
      ondelete={(rows) => rows.forEach(unignore)}
    />
  </div>
  <div class="main">
    <h3>Banned IP Addresses</h3>
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        placeholder="192.168.1.1 or 10.0.*.*"
        bind:value={ipPattern}
        on:input={() => (ipError = '')}
        on:keydown={onEnter(addIpBan)}
      />
      <button on:click={addIpBan}>Ban</button>
    </div>
    {#if ipError}<span class="notice">{ipError}</span>{/if}
    <DataTable
      id="ip-bans"
      label="Banned IP addresses"
      rows={ipBans}
      key={(pattern) => pattern}
      columns={nameColumn('Address or Pattern')}
      headless
      copyText={(pattern) => pattern}
      empty="No addresses are banned."
      onmenu={removal('Remove IP Ban', removeIpBans, false)}
      ondelete={removeIpBans}
    />
  </div>
</div>
