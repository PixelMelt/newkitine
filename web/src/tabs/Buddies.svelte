<script>
  import { buddies, banned, ignored } from '../lib/stores.js';
  import { post, del, formatSize } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  let buddyName = '';
  let banName = '';
  let ignoreName = '';
  let sort = { key: null, dir: 1 };

  $: list = sortRows(
    Object.values($buddies).sort((a, b) => a.username.localeCompare(b.username)),
    sort,
    { speed: (b) => b.stats.avgspeed, files: (b) => b.stats.files },
  );

  function add(path, username, reset) {
    if (!username.trim()) return;
    post(path, { username: username.trim() });
    reset();
  }

  function buddyMenu(event, buddy) {
    openMenu(event, [
      ...userMenu(buddy.username, { skip: ['userlist'] }),
      { sep: true },
      {
        label: 'Add User Note…',
        action: () => {
          const note = prompt(`Note for ${buddy.username}:`, buddy.note);
          if (note !== null) {
            post(`/buddies/${encodeURIComponent(buddy.username)}/note`, { note });
          }
        },
      },
      { sep: true },
      { label: 'Remove', action: () => del(`/buddies/${encodeURIComponent(buddy.username)}`) },
    ]);
  }
</script>

<div class="toolbar">
  <input
    placeholder="Add buddy…"
    bind:value={buddyName}
    on:keydown={(e) => e.key === 'Enter' && add('/buddies', buddyName, () => (buddyName = ''))}
  />
  <button on:click={() => add('/buddies', buddyName, () => (buddyName = ''))}>Add</button>
</div>

<div class="scroll" tabindex="0">
  <table>
    <thead>
      <tr>
        <Th bind:sort key="username" grow>User</Th>
        <Th bind:sort key="status">Status</Th>
        <Th bind:sort key="speed">Speed</Th>
        <Th bind:sort key="files">Files</Th>
        <Th bind:sort key="privileged">Privileged</Th>
        <Th bind:sort key="note" grow>Note</Th>
      </tr>
    </thead>
    <tbody>
      {#each list as buddy (buddy.username)}
        <tr on:contextmenu={(e) => buddyMenu(e, buddy)}>
          <td class="grow">{buddy.username}</td>
          <td>{buddy.status}</td>
          <td>{formatSize(buddy.stats.avgspeed)}/s</td>
          <td>{buddy.stats.files}</td>
          <td>{buddy.privileged ? 'yes' : ''}</td>
          <td class="grow">{buddy.note}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<div class="split" style="flex: 0 0 auto;">
  <div class="side">
    <h3>Banned</h3>
    <div class="toolbar">
      <input
        style="min-width: 0; flex: 1;"
        bind:value={banName}
        on:keydown={(e) => e.key === 'Enter' && add('/banned', banName, () => (banName = ''))}
      />
      <button on:click={() => add('/banned', banName, () => (banName = ''))}>Ban</button>
    </div>
    <div class="list" style="max-height: 150px;">
      {#each $banned as user}
        <div class="row" on:contextmenu={(e) => openMenu(e, userMenu(user))}>
          {user}
          <button on:click={() => del(`/banned/${encodeURIComponent(user)}`)}>Remove</button>
        </div>
      {/each}
    </div>
  </div>
  <div class="side">
    <h3>Ignored</h3>
    <div class="toolbar">
      <input
        style="min-width: 0; flex: 1;"
        bind:value={ignoreName}
        on:keydown={(e) =>
          e.key === 'Enter' && add('/ignored', ignoreName, () => (ignoreName = ''))}
      />
      <button on:click={() => add('/ignored', ignoreName, () => (ignoreName = ''))}>Ignore</button>
    </div>
    <div class="list" style="max-height: 150px;">
      {#each $ignored as user}
        <div class="row" on:contextmenu={(e) => openMenu(e, userMenu(user))}>
          {user}
          <button on:click={() => del(`/ignored/${encodeURIComponent(user)}`)}>Remove</button>
        </div>
      {/each}
    </div>
  </div>
</div>
