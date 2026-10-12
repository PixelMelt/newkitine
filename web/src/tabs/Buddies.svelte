<script>
  import { tick } from 'svelte';
  import { buddies, banned, ignored, ipBans, loadIpBans } from '../lib/stores.js';
  import { route, openUserInfo, noAutofill, focusPane } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { formatSpeed, plural } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenuAt } from '../lib/menu.js';
  import {
    userMenu, addBuddy, removeBuddies, ban, unbanAll, ignore, unignoreAll,
  } from '../lib/usermenu.js';
  import { numeric } from '../lib/columns.js';
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
    numeric('speed', 'Speed', 96, (b) => b.stats.avgspeed, formatSpeed),
    numeric('files', 'Files', 84, (b) => b.stats.files),
    { key: 'privileged', label: 'Privileged', width: 84, text: (b) => (b.privileged ? 'Yes' : '') },
    { key: 'note', label: 'Note', grow: 2, min: 140, text: (b) => b.note },
  ];
  const nameColumn = (label) => [{ key: 'name', label, grow: 1, min: 100, text: (name) => name }];

  const adders = { buddy: addBuddy, banned: ban, ignored: ignore };
  const blockLists = [
    {
      kind: 'banned', title: 'Banned Users', label: 'Banned users', placeholder: 'Name to ban…',
      verb: 'Ban', empty: 'Nobody is banned.', undo: 'Unban', removeAll: unbanAll,
    },
    {
      kind: 'ignored', title: 'Ignored Users', label: 'Ignored users', placeholder: 'Name to ignore…',
      verb: 'Ignore', empty: 'Nobody is ignored.', undo: 'Stop Ignoring', removeAll: unignoreAll,
    },
  ];
  let names = { buddy: '', banned: '', ignored: '' };
  let ipPattern = '';
  let ipError = '';
  let editing = null;
  let noteText = '';
  let noteInput;

  const byName = (a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' });

  $: list = [...$buddies.values()].sort((a, b) => byName(a.username, b.username));
  $: blocked = { banned: [...$banned].sort(byName), ignored: [...$ignored].sort(byName) };
  $: if ($route.tab === 'buddies') loadIpBans();

  async function submit(kind) {
    const username = names[kind].trim();
    if (username && (await adders[kind](username)) && names[kind].trim() === username) names[kind] = '';
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
    const note = noteText;
    const saved = await act(
      () => post(`/buddies/${encodeURIComponent(username)}/note`, { note }),
      { failed: `Note for ${username} not saved` },
    );
    if (saved && editing === username && noteText === note) {
      editing = null;
      focusPane('buddies');
    }
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
      if (ipPattern.trim() === pattern) ipPattern = '';
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
    bind:value={names.buddy}
    on:keydown={onEnter(() => submit('buddy'))}
  />
  <button on:click={() => submit('buddy')}>Add Buddy</button>
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
  ondelete={(rows) => removeBuddies(rows.map((b) => b.username))}
/>

<div class="split short">
  {#each blockLists as { kind, title, label, placeholder, verb, empty, undo, removeAll }}
    <div class="main">
      <h3>{title}</h3>
      <div class="toolbar">
        <input {...noAutofill}
          class="fill"
          {placeholder}
          bind:value={names[kind]}
          on:keydown={onEnter(() => submit(kind))}
        />
        <button on:click={() => submit(kind)}>{verb}</button>
      </div>
      <DataTable
        id={kind}
        {label}
        rows={blocked[kind]}
        key={(name) => name}
        columns={nameColumn('User')}
        headless
        typeahead={(name) => name}
        copyText={(name) => name}
        {empty}
        onactivate={(rows) => openUserInfo(rows[0])}
        onmenu={removal(undo, removeAll, true)}
        ondelete={removeAll}
      />
    </div>
  {/each}
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
      rows={$ipBans}
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
