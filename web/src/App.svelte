<script>
  import { onMount } from 'svelte';
  import {
    connectWebSocket, loadInitialState, status, notices, speedTotals,
  } from './lib/stores.js';
  import { formatSize } from './lib/api.js';
  import { activeTab } from './lib/ui.js';
  import { post } from './lib/api.js';
  import ContextMenu from './lib/ContextMenu.svelte';
  import Search from './tabs/Search.svelte';
  import Downloads from './tabs/Downloads.svelte';
  import Uploads from './tabs/Uploads.svelte';
  import Browse from './tabs/Browse.svelte';
  import UserInfo from './tabs/UserInfo.svelte';
  import PrivateChat from './tabs/PrivateChat.svelte';
  import ChatRooms from './tabs/ChatRooms.svelte';
  import Interests from './tabs/Interests.svelte';
  import Buddies from './tabs/Buddies.svelte';
  import Settings from './tabs/Settings.svelte';

  const tabs = [
    ['search', 'Search Files', Search],
    ['downloads', 'Downloads', Downloads],
    ['uploads', 'Uploads', Uploads],
    ['browse', 'Browse Shares', Browse],
    ['userinfo', 'User Info', UserInfo],
    ['chat', 'Private Chat', PrivateChat],
    ['rooms', 'Chat Rooms', ChatRooms],
    ['interests', 'Interests', Interests],
    ['buddies', 'Buddies', Buddies],
    ['settings', 'Preferences', Settings],
  ];

  onMount(() => {
    loadInitialState();
    connectWebSocket();
  });
</script>

<div style="display: flex; flex-direction: column; height: 100vh;">
  <div class="tabs">
    {#each tabs as [id, label]}
      <button
        class:active={$activeTab === id}
        style:margin-left={id === 'settings' ? 'auto' : null}
        on:click={() => activeTab.set(id)}
      >
        {label}
      </button>
    {/each}
  </div>

  {#each tabs as [id, , component]}
    <div class="pane" style:display={$activeTab === id ? 'flex' : 'none'}>
      <svelte:component this={component} />
    </div>
  {/each}

  <div class="statusbar">
    <span>
      {#if $status.logged_in}
        Connected as {$status.username}
      {:else if $status.connected}
        Connecting…
      {:else}
        Disconnected
        <button on:click={() => post('/connect')}>Connect</button>
      {/if}
    </span>
    <span>Port: {$status.listen_port}</span>
    <span>Shares: {$status.shared_files ?? 0} files / {$status.shared_folders ?? 0} folders</span>
    <span>↓ {formatSize($speedTotals.down)}/s</span>
    <span>↑ {formatSize($speedTotals.up)}/s</span>
    {#if $notices.length}
      <span class="notice">{$notices[$notices.length - 1].text}</span>
    {/if}
  </div>

  <ContextMenu />
</div>
