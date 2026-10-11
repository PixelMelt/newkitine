<script>
  import { onMount, tick } from 'svelte';
  import {
    connectWebSocket, link, status, loginFailure, shownReport, dismissReport,
    speedTotals, unread,
  } from './lib/stores.js';
  import { formatSpeed, plural } from './lib/format.js';
  import { route, go, hrefOf, focusPane } from './lib/ui.js';
  import { post } from './lib/api.js';
  import { act } from './lib/act.js';
  import { isTyping, hasCommand } from './lib/keys.js';
  import { menu } from './lib/menu.js';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import ContextMenu from './lib/ContextMenu.svelte';
  import Search from './tabs/Search.svelte';
  import Transfers from './tabs/Transfers.svelte';
  import Browse from './tabs/Browse.svelte';
  import UserInfo from './tabs/UserInfo.svelte';
  import PrivateChat from './tabs/PrivateChat.svelte';
  import ChatRooms from './tabs/ChatRooms.svelte';
  import Interests from './tabs/Interests.svelte';
  import Buddies from './tabs/Buddies.svelte';
  import Statistics from './tabs/Statistics.svelte';
  import Settings from './tabs/Settings.svelte';

  const tabs = [
    ['search', 'Search Files', 'Search', Search, {}],
    ['downloads', 'Downloads', 'Downloads', Transfers, { direction: 'download' }],
    ['uploads', 'Uploads', 'Uploads', Transfers, { direction: 'upload' }],
    ['browse', 'Browse Shares', 'Browse', Browse, {}],
    ['user', 'User Profiles', 'Profiles', UserInfo, {}],
    ['chat', 'Private Chat', 'Chat', PrivateChat, {}],
    ['rooms', 'Chat Rooms', 'Rooms', ChatRooms, {}],
    ['interests', 'Interests', 'Interests', Interests, {}],
    ['buddies', 'Buddies', 'Buddies', Buddies, {}],
    ['statistics', 'Statistics', 'Stats', Statistics, {}],
    ['settings', 'Preferences', 'Prefs', Settings, {}],
  ];
  let lastTab = null;
  let queryWanted = false;

  $: live = $link.state === 'live';
  $: unreadChats = [...$unread.keys()].filter((key) => key.startsWith('chat:'));
  $: mentions = [...$unread].filter(([key, value]) => key.startsWith('room:') && value.mention);
  $: attention =
    unreadChats.length === 1
      ? `● ${unreadChats[0].slice(5)} · `
      : unreadChats.length
        ? `● ${unreadChats.length} chats · `
        : mentions.length
          ? `● ${mentions[0][0].slice(5)} · `
          : '';
  $: document.title = `${attention}Newkitine${live ? '' : ' (offline)'}`;
  $: arrived($route);

  async function arrived(current) {
    if (current.tab === lastTab) return;
    lastTab = current.tab;
    await tick();
    focusPane(current.tab);
    if (queryWanted) focusQuery();
  }

  function focusQuery() {
    queryWanted = false;
    const query = document.querySelector('[data-search-query]');
    query.focus();
    query.select();
  }

  function focusSearch() {
    if ($route.tab === 'search') return focusQuery();
    queryWanted = true;
    go('search');
  }

  function keydown(event) {
    if ($menu) return;
    if (hasCommand(event) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      focusSearch();
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (isTyping(event)) {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      focusPane($route.tab);
      if (document.activeElement === event.target) event.target.blur();
      return;
    }
    if (event.key === '/' && !event.repeat) {
      event.preventDefault();
      focusSearch();
    }
  }

  function tabClick(event, id) {
    if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault();
    go(id);
  }

  const connect = () => act(() => post('/connect'), { failed: 'Not connected to Soulseek' });

  onMount(connectWebSocket);
</script>

<svelte:window on:keydown={keydown} />

<div class="app">
  <nav class="tabs" aria-label="Sections">
    {#each tabs as [id, label, short]}
      <a
        href={hrefOf($route, id)}
        class:active={$route.tab === id}
        class:last={id === 'settings'}
        class:unread={(id === 'chat' && unreadChats.length) || (id === 'rooms' && mentions.length)}
        aria-current={$route.tab === id ? 'page' : null}
        on:click={(e) => tabClick(e, id)}
      >
        <span class="long">{label}</span><span class="short">{short}</span>
      </a>
    {/each}
  </nav>

  {#each tabs as [id, , , component, props]}
    <div class="pane" class:stale={!live} data-tab={id} hidden={$route.tab !== id}>
      <svelte:component this={component} {...props} />
    </div>
  {/each}

  <div class="statusbar">
    {#if $link.state === 'connecting'}
      <span>Connecting to the Newkitine server…</span>
    {:else if $link.state === 'lost'}
      <span class="notice">
        {$link.known ? 'Lost the Newkitine server' : "Can't reach the Newkitine server"}, retrying
        every 3 seconds
      </span>
      <button on:click={connectWebSocket}>Retry Now</button>
    {:else if $link.state === 'broken'}
      <span class="notice" title={$link.error}>This page no longer matches the server.</span>
      <button on:click={() => location.reload()}>Reload</button>
    {:else if $status.logged_in}
      <span>Connected as {$status.username}</span>
    {:else if $status.connected}
      <span>Logging in to Soulseek…</span>
    {:else}
      <span class:notice={$loginFailure}>
        {$loginFailure ? `Soulseek login failed: ${$loginFailure}` : 'Not connected to Soulseek'}
      </span>
      <button on:click={connect}>Connect</button>
      {#if $loginFailure}<button on:click={() => go('settings', 'network')}>Preferences</button>{/if}
    {/if}
    {#if $link.known}
      <span class="facts-line" class:dim={!live}>
        <span>Port {$status.listen_port}</span>
        {#if $status.scanning}
          <span>
            Scanning shares…
            {#if $status.scan_progress}{plural($status.scan_progress, 'file')}{/if}
          </span>
        {:else if $status.share_scan_error}
          <span class="notice" title={$status.share_scan_error}>Share scan failed</span>
        {:else}
          <span>
            Sharing {plural($status.shared_files, 'file')} in {plural($status.shared_folders, 'folder')}
          </span>
        {/if}
        <span class="conns">{plural($status.peer_connections, 'connection')}</span>
        <span class="speed"><ArrowDown size={12} /> {live ? formatSpeed($speedTotals.down) : '–'}</span>
        <span class="speed"><ArrowUp size={12} /> {live ? formatSpeed($speedTotals.up) : '–'}</span>
      </span>
    {/if}
    <span class="report {$shownReport ? $shownReport.kind : ''}" role="status" title={$shownReport?.text}>
      {$shownReport ? $shownReport.text : ''}
    </span>
    {#if $shownReport}
      {#if $shownReport.action}
        <button
          on:click={() => {
            const { run } = $shownReport.action;
            dismissReport();
            run();
          }}
        >
          {$shownReport.action.label}
        </button>
      {/if}
    {/if}
  </div>

  <ContextMenu />
</div>
