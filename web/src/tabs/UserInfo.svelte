<script>
  import { userInfos, interests } from '../lib/stores.js';
  import { userInfoTarget, openSearch } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { formatSize } from '../lib/format.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';

  let selected = null;
  let newUser = '';
  let closed = {};

  $: users = Object.keys($userInfos).filter((user) => !closed[user]).sort();
  $: info = selected ? $userInfos[selected] : null;

  $: if ($userInfoTarget) {
    const target = $userInfoTarget;
    userInfoTarget.set(null);
    show(target);
  }

  function show(username) {
    selected = username;
    delete closed[username];
    closed = closed;
    if (!$userInfos[username]) request(username);
  }

  function request(username) {
    post(`/users/${encodeURIComponent(username)}/info`);
  }

  function openNew() {
    if (!newUser.trim()) return;
    show(newUser.trim());
    newUser = '';
  }

  function close(username) {
    closed[username] = true;
    closed = closed;
    if (selected === username) selected = null;
  }

  function entryMenu(event, username) {
    openMenu(event, [
      ...userMenu(username, { skip: ['userinfo'] }),
      { sep: true },
      { label: 'Close', action: () => close(username) },
    ]);
  }

  function interestMenu(event, thing) {
    const isLiked = $interests.liked.includes(thing);
    const isHated = $interests.hated.includes(thing);
    openMenu(event, [
      {
        label: 'I Like This',
        checked: isLiked,
        action: () =>
          post(`/interests/${isLiked ? 'remove' : 'add'}`, { kind: 'liked', thing }),
      },
      {
        label: 'I Dislike This',
        checked: isHated,
        action: () =>
          post(`/interests/${isHated ? 'remove' : 'add'}`, { kind: 'hated', thing }),
      },
      { sep: true },
      { label: 'Recommendations for Item', action: () => post('/interests/item', { thing }) },
      { label: 'Search for Item', action: () => openSearch(thing) },
    ]);
  }
</script>

<div class="split">
  <div class="side">
    <div class="toolbar">
      <input
        style="min-width: 0; flex: 1;"
        placeholder="Username…"
        bind:value={newUser}
        on:keydown={(e) => e.key === 'Enter' && openNew()}
      />
      <button on:click={openNew}>Open</button>
    </div>
    <div class="list" tabindex="0">
      {#each users as user}
        <div
          class:selected={selected === user}
          on:click={() => show(user)}
          on:contextmenu={(e) => entryMenu(e, user)}
        >
          {user}
        </div>
      {/each}
    </div>
  </div>
  <div class="main">
    {#if info}
      <div class="toolbar">
        <h3 style="margin: 0; flex: 1;">{info.username}</h3>
        <button on:click={() => request(info.username)}>Refresh</button>
      </div>
      <div class="split" style="overflow: auto;">
        <div class="main" style="overflow: auto;">
          <h3>Description</h3>
          <pre class="description">{info.received ? info.description : 'Waiting for response…'}</pre>
          {#if info.picture_base64}
            <img
              src="data:image;base64,{info.picture_base64}"
              alt="{info.username}'s picture"
              style="max-width: 300px;"
            />
          {/if}
          <h3>Interests</h3>
          <div class="split" style="flex: 0 0 auto; min-height: 100px; max-height: 200px;">
            <div class="main">
              <span>Likes</span>
              <div class="list">
                {#each info.interests_liked as thing}
                  <div on:contextmenu={(e) => interestMenu(e, thing)}>{thing}</div>
                {/each}
              </div>
            </div>
            <div class="main">
              <span>Dislikes</span>
              <div class="list">
                {#each info.interests_hated as thing}
                  <div on:contextmenu={(e) => interestMenu(e, thing)}>{thing}</div>
                {/each}
              </div>
            </div>
          </div>
        </div>
        <div class="side">
          <table style="width: auto;">
            <tbody>
              <tr>
                <td>Shared Files</td>
                <td>{info.stats ? info.stats.files.toLocaleString() : 'Unknown'}</td>
              </tr>
              <tr>
                <td>Shared Folders</td>
                <td>{info.stats ? info.stats.dirs.toLocaleString() : 'Unknown'}</td>
              </tr>
              <tr>
                <td>Upload Speed</td>
                <td>{info.stats ? formatSize(info.stats.avgspeed) + '/s' : 'Unknown'}</td>
              </tr>
              <tr>
                <td>Upload Slot Available</td>
                <td>{info.received ? (info.slots_available ? 'Yes' : 'No') : 'Unknown'}</td>
              </tr>
              <tr>
                <td>Upload Slots</td>
                <td>{info.received ? info.upload_slots : 'Unknown'}</td>
              </tr>
              <tr>
                <td>Queued Uploads</td>
                <td>{info.received ? info.queue_size : 'Unknown'}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    {:else}
      <span>Open a user profile.</span>
    {/if}
  </div>
</div>
