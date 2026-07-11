<script>
  import { interests } from '../lib/stores.js';
  import { activeTab, openSearch, openUserInfo } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  let liked = '';
  let hated = '';
  let populated = false;
  let recSort = { key: null, dir: 1 };
  let similarSort = { key: null, dir: 1 };

  $: if ($activeTab === 'interests' && !populated) {
    populated = true;
    post('/interests/refresh');
  }

  $: recommendations = sortRows(
    [...$interests.recommendations, ...$interests.unrecommendations].sort((a, b) => b[1] - a[1]),
    recSort,
    { item: (r) => r[0], rating: (r) => r[1] },
  );
  $: similarUsers = sortRows($interests.similar_users, similarSort, {
    rating: (u) => u.rating ?? 0,
  });
  $: recommendationsTitle = $interests.recommendations_global
    ? 'Popular Interests'
    : $interests.recommendations_for
      ? `Recommendations for ${$interests.recommendations_for}`
      : 'Recommendations';
  $: similarTitle = $interests.similar_users_for
    ? `Users who like ${$interests.similar_users_for}`
    : 'Similar Users';

  function add(kind, thing) {
    if (!thing.trim()) return;
    post('/interests/add', { kind, thing: thing.trim() });
    liked = '';
    hated = '';
  }

  function remove(kind, thing) {
    post('/interests/remove', { kind, thing });
  }

  function recommendItem(thing) {
    post('/interests/item', { thing });
  }

  function interestMenu(event, kind, thing) {
    openMenu(event, [
      { label: 'Recommendations for Item', action: () => recommendItem(thing) },
      { label: 'Search for Item', action: () => openSearch(thing) },
      { sep: true },
      { label: 'Remove', action: () => remove(kind, thing) },
    ]);
  }

  function recommendationMenu(event, thing) {
    const isLiked = $interests.liked.includes(thing);
    const isHated = $interests.hated.includes(thing);
    openMenu(event, [
      {
        label: 'I Like This',
        checked: isLiked,
        action: () => (isLiked ? remove('liked', thing) : add('liked', thing)),
      },
      {
        label: 'I Dislike This',
        checked: isHated,
        action: () => (isHated ? remove('hated', thing) : add('hated', thing)),
      },
      { sep: true },
      { label: 'Recommendations for Item', action: () => recommendItem(thing) },
      { label: 'Search for Item', action: () => openSearch(thing) },
    ]);
  }
</script>

<div class="toolbar">
  <button on:click={() => post('/interests/refresh')}>Refresh Recommendations</button>
</div>

<div class="split">
  <div class="side">
    <h3>Personal Interests</h3>
    <div class="toolbar">
      <input
        style="min-width: 0; flex: 1;"
        placeholder="Add something you like…"
        bind:value={liked}
        on:keydown={(e) => e.key === 'Enter' && add('liked', liked)}
      />
      <button on:click={() => add('liked', liked)}>Add</button>
    </div>
    <div class="list">
      {#each $interests.liked as thing}
        <div class="row" on:contextmenu={(e) => interestMenu(e, 'liked', thing)}>
          {thing}
          <button on:click={() => remove('liked', thing)}>Remove</button>
        </div>
      {/each}
    </div>
    <h3>Personal Dislikes</h3>
    <div class="toolbar">
      <input
        style="min-width: 0; flex: 1;"
        placeholder="Add something you dislike…"
        bind:value={hated}
        on:keydown={(e) => e.key === 'Enter' && add('hated', hated)}
      />
      <button on:click={() => add('hated', hated)}>Add</button>
    </div>
    <div class="list">
      {#each $interests.hated as thing}
        <div class="row" on:contextmenu={(e) => interestMenu(e, 'hated', thing)}>
          {thing}
          <button on:click={() => remove('hated', thing)}>Remove</button>
        </div>
      {/each}
    </div>
  </div>
  <div class="main">
    <h3>{recommendationsTitle}</h3>
    <div class="scroll" tabindex="0">
      <table>
        <thead>
          <tr>
            <Th bind:sort={recSort} key="item" grow>Item</Th>
            <Th bind:sort={recSort} key="rating">Rating</Th>
          </tr>
        </thead>
        <tbody>
          {#each recommendations as [thing, rating]}
            <tr
              class="clickable"
              on:contextmenu={(e) => recommendationMenu(e, thing)}
              on:dblclick={() => recommendItem(thing)}
            >
              <td class="grow">{thing}</td>
              <td>{rating.toLocaleString()}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <h3>{similarTitle}</h3>
    <div class="scroll" tabindex="0" style="max-height: 200px;">
      <table>
        <thead>
          <tr>
            <Th bind:sort={similarSort} key="username" grow>User</Th>
            <Th bind:sort={similarSort} key="rating">Rating</Th>
          </tr>
        </thead>
        <tbody>
          {#each similarUsers as user}
            <tr
              class="clickable"
              on:contextmenu={(e) => openMenu(e, userMenu(user.username))}
              on:dblclick={() => openUserInfo(user.username)}
            >
              <td class="grow">{user.username}</td>
              <td>{user.rating || ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
</div>
