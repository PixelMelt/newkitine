<script>
  import { interests, status, openSearch } from '../lib/stores.js';
  import { route, openUserInfo, noAutofill } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { formatCount, plural } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';

  const thingColumn = (label) => [{ key: 'thing', label, grow: 1, min: 100, text: (thing) => thing }];
  const recommendationColumns = [
    { key: 'item', label: 'Item', grow: 1, min: 160, text: (r) => r[0] },
    {
      key: 'rating', label: 'Rating', width: 84, align: 'end', desc: true,
      text: (r) => formatCount(r[1]), sortValue: (r) => r[1],
    },
  ];
  const similarColumns = [
    { key: 'username', label: 'User', grow: 1, min: 160, text: (u) => u.username },
    {
      key: 'rating', label: 'Rating', width: 84, align: 'end', desc: true,
      text: (u) => (u.rating ? formatCount(u.rating) : ''), sortValue: (u) => u.rating,
    },
  ];

  let liked = '';
  let hated = '';
  let populated = false;

  $: if ($route.tab === 'interests' && $status.logged_in && !populated) {
    populated = true;
    refresh();
  }
  $: recommendations = [...$interests.recommendations, ...$interests.unrecommendations].sort(
    (a, b) => b[1] - a[1],
  );
  $: recommendationsTitle = $interests.recommendations_global
    ? 'Popular Interests'
    : $interests.recommendations_for
      ? `Recommendations for ${$interests.recommendations_for}`
      : 'Recommendations';
  $: similarTitle = $interests.similar_users_for
    ? `Users Who Like ${$interests.similar_users_for}`
    : 'Similar Users';

  const refresh = () =>
    act(() => post('/interests/refresh'), { failed: 'Recommendations not refreshed' });

  const add = (kind, thing) =>
    act(() => post('/interests/add', { kind, thing }), { failed: `"${thing}" not added` });

  const remove = (kind, thing) =>
    act(() => post('/interests/remove', { kind, thing }), {
      failed: `"${thing}" not removed`,
      done: `Removed "${thing}"`,
      action: { label: 'Undo', run: () => add(kind, thing) },
    });

  function removeAll(kind, things) {
    if (things.length === 1) return remove(kind, things[0]);
    const change = (path) => () =>
      Promise.all(things.map((thing) => post(`/interests/${path}`, { kind, thing })));
    return act(change('remove'), {
      failed: 'Interests not removed',
      done: `Removed ${plural(things.length, 'interest')}`,
      action: {
        label: 'Undo',
        run: () => act(change('add'), { failed: 'Interests not restored' }),
      },
    });
  }

  const recommendItem = (thing) =>
    act(() => post('/interests/item', { thing }), {
      failed: `Recommendations for "${thing}" not requested`,
    });

  async function addLiked() {
    const thing = liked.trim();
    if (thing && (await add('liked', thing)) && liked.trim() === thing) liked = '';
  }

  async function addHated() {
    const thing = hated.trim();
    if (thing && (await add('hated', thing)) && hated.trim() === thing) hated = '';
  }

  const ownMenu = (kind) => (rows, point) =>
    openMenuAt(point, [
      {
        label: 'Recommendations for Item', primary: true,
        action: () => recommendItem(rows[0]),
      },
      { label: 'Search for Item', action: () => openSearch(rows[0]) },
      { sep: true },
      { label: 'Remove', action: () => removeAll(kind, rows) },
    ]);

  function recommendationMenu(rows, point) {
    const thing = rows[0][0];
    const isLiked = $interests.liked.includes(thing);
    const isHated = $interests.hated.includes(thing);
    openMenuAt(point, [
      {
        label: 'Recommendations for Item', primary: true,
        action: () => recommendItem(thing),
      },
      { label: 'Search for Item', action: () => openSearch(thing) },
      { sep: true },
      {
        label: 'I Like This', checked: isLiked,
        action: () => (isLiked ? remove('liked', thing) : add('liked', thing)),
      },
      {
        label: 'I Dislike This', checked: isHated,
        action: () => (isHated ? remove('hated', thing) : add('hated', thing)),
      },
    ]);
  }
</script>

<div class="toolbar">
  <button on:click={refresh}>Refresh Recommendations</button>
  {#if !$status.logged_in}
    <span class="hint">Not connected to Soulseek.</span>
  {/if}
</div>

<div class="split">
  <div class="side wide">
    <h3>Things I Like</h3>
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        placeholder="Add something you like…"
        bind:value={liked}
        on:keydown={onEnter(addLiked)}
      />
      <button on:click={addLiked}>Add</button>
    </div>
    <DataTable
      id="liked"
      label="Things I like"
      rows={$interests.liked}
      key={(thing) => thing}
      columns={thingColumn('Like')}
      headless
      typeahead={(thing) => thing}
      copyText={(thing) => thing}
      empty="Nothing yet."
      onactivate={(rows) => recommendItem(rows[0])}
      onmenu={ownMenu('liked')}
      ondelete={(rows) => removeAll('liked', rows)}
    />
    <h3>Things I Dislike</h3>
    <div class="toolbar">
      <input {...noAutofill}
        class="fill"
        placeholder="Add something you dislike…"
        bind:value={hated}
        on:keydown={onEnter(addHated)}
      />
      <button on:click={addHated}>Add</button>
    </div>
    <DataTable
      id="hated"
      label="Things I dislike"
      rows={$interests.hated}
      key={(thing) => thing}
      columns={thingColumn('Dislike')}
      headless
      typeahead={(thing) => thing}
      copyText={(thing) => thing}
      empty="Nothing yet."
      onactivate={(rows) => recommendItem(rows[0])}
      onmenu={ownMenu('hated')}
      ondelete={(rows) => removeAll('hated', rows)}
    />
  </div>
  <div class="main">
    <h3>{recommendationsTitle}</h3>
    <DataTable
      id="recommendations"
      label={recommendationsTitle}
      rows={recommendations}
      key={(r) => `${r[0]}\u0000${r[1]}`}
      columns={recommendationColumns}
      primary
      typeahead={(r) => r[0]}
      copyText={(r) => r[0]}
      empty="No recommendations yet."
      onactivate={(rows) => recommendItem(rows[0][0])}
      onmenu={recommendationMenu}
    />
    <h3>{similarTitle}</h3>
    <DataTable
      id="similar-users"
      label={similarTitle}
      rows={$interests.similar_users}
      key={(u) => u.username}
      columns={similarColumns}
      typeahead={(u) => u.username}
      copyText={(u) => u.username}
      empty="No similar users yet."
      onactivate={(rows) => openUserInfo(rows[0].username)}
      onmenu={(rows, point) => openMenuAt(point, userMenu(rows[0].username, { primary: true }))}
    />
  </div>
</div>
