<script>
  import { rooms, status } from '../lib/stores.js';
  import { activeTab } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { formatTime } from '../lib/format.js';
  import { autoscroll } from '../lib/autoscroll.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  let roomSort = { key: null, dir: 1 };
  let selected = null;
  let newRoom = '';
  let draft = '';
  let showAvailable = false;
  let unread = {};
  let counts = {};

  $: joinedNames = Object.keys($rooms.joined).sort();
  $: if (!$rooms.joined[selected] && joinedNames.length) selected = joinedNames[0];
  $: current = $rooms.joined[selected] ?? null;

  $: {
    for (const [room, view] of Object.entries($rooms.joined)) {
      const chatMessages = view.messages.length;
      if (counts[room] !== undefined && chatMessages > counts[room]
          && !(room === selected && $activeTab === 'rooms' && !showAvailable)) {
        unread[room] = true;
      }
      counts[room] = chatMessages;
    }
  }
  $: if ($activeTab === 'rooms' && selected && !showAvailable && unread[selected]) {
    delete unread[selected];
    unread = unread;
  }

  function join(room) {
    if (!room.trim()) return;
    post('/rooms/join', { room: room.trim() });
    selected = room.trim();
    newRoom = '';
    showAvailable = false;
  }

  function select(room) {
    selected = room;
    showAvailable = false;
    delete unread[room];
    unread = unread;
  }

  function leave(room) {
    post('/rooms/leave', { room });
  }

  function send() {
    if (!draft.trim() || !selected) return;
    post(`/rooms/${encodeURIComponent(selected)}/messages`, { message: draft });
    draft = '';
  }
</script>

<div class="toolbar">
  <input
    placeholder="Join room…"
    bind:value={newRoom}
    on:keydown={(e) => e.key === 'Enter' && join(newRoom)}
  />
  <button on:click={() => join(newRoom)}>Join</button>
  <button class:active={showAvailable} on:click={() => (showAvailable = !showAvailable)}>
    Room List ({$rooms.available.length})
  </button>
</div>

{#if joinedNames.length}
  <div class="subtabs">
    {#each joinedNames as room}
      <span class="subtab">
        <button
          class:active={selected === room && !showAvailable}
          class:unread={unread[room]}
          on:click={() => select(room)}
          on:contextmenu={(e) =>
            openMenu(e, [{ label: 'Leave Room', action: () => leave(room) }])}
        >
          {room}
        </button>
        <button on:click={() => leave(room)}>Leave</button>
      </span>
    {/each}
  </div>
{/if}

{#if showAvailable}
  <div class="scroll" tabindex="0">
    <table>
      <thead>
        <tr>
          <Th bind:sort={roomSort} key="name" grow>Room</Th>
          <Th bind:sort={roomSort} key="users">Users</Th>
        </tr>
      </thead>
      <tbody>
        {#each sortRows($rooms.available, roomSort) as room}
          <tr
            class="clickable"
            on:dblclick={() => join(room.name)}
            on:contextmenu={(e) =>
              openMenu(e, [{ label: 'Join Room', action: () => join(room.name) }])}
          >
            <td class="grow">{room.name}</td>
            <td>{room.users}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <span>Double-click a room to join it.</span>
{:else if current}
  <div class="split">
    <div class="main">
      <div class="messages" tabindex="0" use:autoscroll={current.messages}>
        {#each current.messages as message}
          <div class="message">
            <span class="time">{formatTime(message.timestamp)}</span>
            <span
              class="sender"
              class:self={message.sender === $status.username}
              on:contextmenu={(e) => openMenu(e, userMenu(message.sender))}
            >
              {message.sender}:
            </span>
            <span>{message.message}</span>
          </div>
        {/each}
      </div>
      <div class="toolbar">
        <input
          style="flex: 1;"
          placeholder="Message {selected}…"
          bind:value={draft}
          on:keydown={(e) => e.key === 'Enter' && send()}
        />
        <button on:click={send}>Send</button>
      </div>
    </div>
    <div class="side">
      <span>{current.users.length} users</span>
      <div class="list" tabindex="0">
        {#each current.users as user}
          <div on:contextmenu={(e) => openMenu(e, userMenu(user))}>{user}</div>
        {/each}
      </div>
    </div>
  </div>
{:else}
  <span>Join a room to start chatting.</span>
{/if}
