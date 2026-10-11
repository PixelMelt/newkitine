<script>
  import { tick } from 'svelte';
  import { userInfos, interests, openSearch, link } from '../lib/stores.js';
  import { persisted } from '../lib/persisted.js';
  import { route, go, forget, openUserInfo, noAutofill } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { formatSpeed, formatCount, formatTime, segments } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';

  const closed = persisted('closedProfiles', [], Array.isArray);
  const userColumn = [{ key: 'username', label: 'Profile', grow: 1, min: 100, text: (u) => u }];
  const thingColumn = (label) => [{ key: 'thing', label, grow: 1, min: 80, text: (t) => t }];

  let newUser = '';
  let selection = new Set();
  let requests = Object.create(null);

  $: users = [...$userInfos.keys()]
    .filter((user) => !$closed.includes(user))
    .sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }));
  $: wanted = $route.items.user ?? null;
  $: if ($link.state === 'live' && wanted && $route.tab === 'user') {
    const username = wanted;
    tick().then(() => ensure(username));
  }
  $: selected = users.includes(wanted) ? wanted : null;
  $: info = selected ? $userInfos.get(selected) : null;
  $: selection = new Set(selected ? [selected] : []);

  function ensure(username) {
    if ($closed.includes(username)) closed.update((list) => list.filter((u) => u !== username));
    if (!$userInfos.has(username) && !requests[username]) request(username);
  }

  async function request(username) {
    requests[username] = { at: Date.now() / 1000, failed: null };
    await act(() => post(`/users/${encodeURIComponent(username)}/info`), {
      failed: `Profile of ${username} not requested`,
      where: (text) => (requests[username] = { ...requests[username], failed: text }),
    });
  }

  function openNew() {
    const username = newUser.trim();
    if (!username) return;
    delete requests[username];
    openUserInfo(username);
    newUser = '';
  }

  function close(names) {
    closed.update((list) => [...list, ...names]);
    for (const name of names) {
      delete requests[name];
      forget('user', name);
    }
    selection = new Set();
  }

  function entryMenu(rows, point) {
    openMenuAt(point, [
      ...userMenu(rows[0], { current: 'profile' }),
      { sep: true },
      { label: 'Close Profile', action: () => close(rows) },
    ]);
  }

  function interestMenu(rows, point) {
    const thing = rows[0];
    const isLiked = $interests.liked.includes(thing);
    const isHated = $interests.hated.includes(thing);
    const toggle = (kind, on) =>
      act(() => post(`/interests/${on ? 'remove' : 'add'}`, { kind, thing }), {
        failed: `"${thing}" not changed`,
      });
    openMenuAt(point, [
      { label: 'Search for Item', primary: true, action: () => openSearch(thing) },
      {
        label: 'Recommendations for Item',
        action: () =>
          act(() => post('/interests/item', { thing }), {
            failed: `Recommendations for "${thing}" not requested`,
          }),
      },
      { sep: true },
      { label: 'I Like This', checked: isLiked, action: () => toggle('liked', isLiked) },
      { label: 'I Dislike This', checked: isHated, action: () => toggle('hated', isHated) },
    ]);
  }
</script>

<div class="split">
  <div class="side">
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        data-primary={users.length ? null : ''}
        placeholder="Open profile of…"
        bind:value={newUser}
        on:keydown={onEnter(openNew)}
      />
      <button on:click={openNew}>Open</button>
    </div>
    <DataTable
      id="profiles"
      label="Open profiles"
      rows={users}
      key={(u) => u}
      columns={userColumn}
      headless
      primary
      bind:selection
      typeahead={(u) => u}
      copyText={(u) => u}
      empty="No profiles open."
      onpick={(user) => go('user', user)}
      onmenu={entryMenu}
      ondelete={close}
    />
  </div>
  <div class="main">
    {#if info}
      <div class="toolbar">
        <h3 class="fill">{info.username}</h3>
        <button on:click={() => request(info.username)}>Refresh</button>
      </div>
      <div class="split scrolling">
        <div class="main">
          <h3>Description</h3>
          {#if info.received}
            <pre class="description">{#each segments(info.description) as part}{#if part.link && !part.text.startsWith('slsk:')}<a href={part.text} target="_blank" rel="noopener noreferrer">{part.text}</a>{:else}{part.text}{/if}{/each}</pre>
          {:else}
            <p class="hint">
              {#if requests[info.username]?.failed}
                {requests[info.username].failed}
              {:else if requests[info.username]}
                Asked {info.username} at {formatTime(requests[info.username].at)}, no answer yet.
                Users who are offline cannot answer.
              {:else}
                No answer from {info.username} yet.
              {/if}
            </p>
          {/if}
          <div class="split short">
            <div class="main">
              <DataTable
                id="profile-likes"
                label="{info.username}'s likes"
                rows={info.interests_liked}
                key={(t) => t}
                columns={thingColumn('Likes')}
                viewKey={info.username}
                copyText={(t) => t}
                empty="None listed."
                onactivate={(rows) => openSearch(rows[0])}
                onmenu={interestMenu}
              />
            </div>
            <div class="main">
              <DataTable
                id="profile-dislikes"
                label="{info.username}'s dislikes"
                rows={info.interests_hated}
                key={(t) => t}
                columns={thingColumn('Dislikes')}
                viewKey={info.username}
                copyText={(t) => t}
                empty="None listed."
                onactivate={(rows) => openSearch(rows[0])}
                onmenu={interestMenu}
              />
            </div>
          </div>
        </div>
        <div class="side">
          {#if info.picture_base64}
            <img
              class="picture"
              src="data:image;base64,{info.picture_base64}"
              alt="{info.username}'s picture"
            />
          {/if}
          <dl class="facts">
            <dt>Shared files</dt>
            <dd>{info.stats ? formatCount(info.stats.files) : 'Unknown'}</dd>
            <dt>Shared folders</dt>
            <dd>{info.stats ? formatCount(info.stats.dirs) : 'Unknown'}</dd>
            <dt>Upload speed</dt>
            <dd>{info.stats ? formatSpeed(info.stats.avgspeed) : 'Unknown'}</dd>
            <dt>Free upload slot</dt>
            <dd>{info.received ? (info.slots_available ? 'Yes' : 'No') : 'Unknown'}</dd>
            <dt>Upload slots</dt>
            <dd>{info.received ? formatCount(info.upload_slots) : 'Unknown'}</dd>
            <dt>Queued uploads</dt>
            <dd>{info.received ? formatCount(info.queue_size) : 'Unknown'}</dd>
          </dl>
        </div>
      </div>
    {:else if wanted && requests[wanted]}
      <p class={requests[wanted].failed ? 'notice' : 'hint'}>
        {requests[wanted].failed ??
          `Asked the server about ${wanted} at ${formatTime(requests[wanted].at)}…`}
      </p>
    {/if}
  </div>
</div>
