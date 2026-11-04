<script>
  import {
    rooms, conversations, unread, roomKey, joinRoom, leaveRoom, status, ignored,
  } from '../lib/stores.js';
  import { route, go, openUserInfo, noAutofill } from '../lib/ui.js';
  import { copy } from '../lib/act.js';
  import {
    plural, fold, formatCount, formatSpeed, countryFlag, countryName,
  } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenu, openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { compareNames } from '../lib/sort.js';
  import { numeric } from '../lib/columns.js';
  import Subtabs from '../lib/Subtabs.svelte';
  import DataTable from '../lib/DataTable.svelte';
  import MessageLog from '../lib/MessageLog.svelte';
  import Composer from '../lib/Composer.svelte';

  let roomName = '';
  let listRequested = false;
  let strip;
  let log;

  const roomColumns = [
    { key: 'name', label: 'Room', grow: 1, min: 200, text: (r) => r.name },
    numeric('users', 'Users', 80, (r) => r.users),
    { key: 'joined', label: '', width: 70, text: (r) => (r.joined ? 'Joined' : ''), class: () => 'dim' },
  ];
  const STATUS = { online: ['Online', 0], away: ['Away', 1], offline: ['Offline', 2], unknown: ['Unknown', 3] };

  const statsHint = ({ stats }) =>
    [
      `Files: ${formatCount(stats.files)}`,
      `Folders: ${formatCount(stats.dirs)}`,
      `Speed: ${stats.avgspeed > 0 ? formatSpeed(stats.avgspeed) : 'Unknown'}`,
    ].join('\n');

  const byFiles = (a, b) => b.stats.files - a.stats.files || compareNames(a.username, b.username);

  $: userColumns = [
    {
      key: 'status', label: '', width: 22, text: (u) => (STATUS[u.status][1] < 2 ? '●' : ''),
      sortValue: (u) => STATUS[u.status][1], class: (u) => u.status, hint: (u) => STATUS[u.status][0],
    },
    {
      key: 'country', label: '', width: 34, text: (u) => countryFlag(u.country),
      sortValue: (u) => countryName(u.country), hint: (u) => countryName(u.country),
    },
    {
      key: 'username', label: 'User', grow: 1, min: 90, text: (u) => u.username,
      class: (u) => ($ignored.includes(u.username) ? 'dim' : ''),
    },
    { ...numeric('files', 'Files', 72, (u) => u.stats.files), hint: statsHint },
  ];

  $: joinedNames = [...$rooms.joined.keys()].sort(compareNames);
  $: wanted = $route.items.rooms;
  $: selected = wanted ? ($rooms.joined.has(wanted) ? wanted : null) : (joinedNames[0] ?? null);
  $: showList = listRequested || !selected;
  $: current = selected ? $rooms.joined.get(selected) : null;
  $: users = current && !showList ? [...current.users].sort(byFiles) : [];
  $: needle = fold(roomName.trim());
  $: available = showList
    ? $rooms.available
        .filter((room) => fold(room.name).includes(needle))
        .map((room) => ({ ...room, joined: $rooms.joined.has(room.name) }))
    : [];
  $: items = joinedNames.map((room) => ({
    id: room,
    label: room,
    unread: $unread.has(roomKey(room)),
  }));

  function show(room) {
    listRequested = false;
    go('rooms', room);
  }

  async function join(room) {
    if (!room) return;
    const typed = roomName;
    if ($rooms.joined.has(room) || (await joinRoom(room))) {
      if (roomName === typed) roomName = '';
      show(room);
    }
  }

  function leave(room) {
    strip.leave(room, selected);
    leaveRoom(room);
  }

  function tabMenu(event, room) {
    openMenu(event, [
      { label: 'Copy Room Name', action: () => copy(room, 'room name') },
      { sep: true },
      { label: 'Leave Room', action: () => leave(room) },
    ]);
  }

  function roomMenu(rows, point) {
    const room = rows[0];
    openMenuAt(point, [
      {
        label: room.joined ? 'Go to Room' : 'Join Room', primary: true,
        action: () => join(room.name),
      },
      { label: 'Copy Room Name', action: () => copy(room.name, 'room name') },
    ]);
  }
</script>

<div class="toolbar">
  <input {...noAutofill}
    placeholder="Find or join room…"
    bind:value={roomName}
    on:focus={() => (listRequested = true)}
    on:input={() => (listRequested = true)}
    on:keydown={onEnter(() => join(roomName.trim()))}
  />
  <button on:click={() => join(roomName.trim())}>Join</button>
  <button
    class:active={showList}
    aria-pressed={showList}
    on:click={() => (listRequested = !showList)}
  >
    Room List
  </button>
  {#if showList}
    <span class="count">
      {needle ? `${formatCount(available.length)} of ` : ''}{plural($rooms.available.length, 'room')}
    </span>
  {/if}
</div>

<Subtabs
  tab="rooms"
  {items}
  active={showList ? null : selected}
  closeLabel="Leave"
  bind:this={strip}
  onselect={show}
  onclose={leave}
  onmenu={tabMenu}
/>

{#if showList}
  <DataTable
    id="rooms"
    label="Available rooms"
    rows={available}
    key={(r) => r.name}
    columns={roomColumns}
    typeahead={(r) => r.name}
    copyText={(r) => r.name}
    empty={$status.logged_in
      ? needle
        ? `No room matches “${roomName.trim()}”. Press Enter to create and join it.`
        : 'The server has not sent the room list yet.'
      : 'Not connected to Soulseek. The room list loads after login.'}
    onactivate={(rows) => join(rows[0].name)}
    onmenu={roomMenu}
  />
{:else}
  <div class="split">
    <div class="main">
      <MessageLog
        key={roomKey(selected)}
        messages={$conversations.get(roomKey(selected)) ?? []}
        self={$status.username}
        active={$route.tab === 'rooms'}
        senderMenu={(e, sender) => openMenu(e, userMenu(sender))}
        bind:this={log}
      />
      <Composer
        key={roomKey(selected)}
        placeholder="Message {selected}…"
        onsent={() => log.toBottom()}
      />
    </div>
    <div class="side roster">
      <span class="count">{plural(users.length, 'user')}</span>
      <DataTable
        id="room-users"
        label="Users in {selected}"
        rows={users}
        key={(u) => u.username}
        columns={userColumns}
        viewKey={selected}
        typeahead={(u) => u.username}
        copyText={(u) => u.username}
        empty="Nobody here."
        onactivate={(rows) => openUserInfo(rows[0].username)}
        onmenu={(rows, point) => openMenuAt(point, userMenu(rows[0].username, { primary: true }))}
      />
    </div>
  </div>
{/if}
