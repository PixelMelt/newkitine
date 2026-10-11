<script>
  import {
    chatPartners, conversations, unread, chatKey, closeChat, openChat, forgetChat, status,
  } from '../lib/stores.js';
  import { route, go, forget, openPrivateChat, noAutofill } from '../lib/ui.js';
  import { del } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { plural } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import Subtabs from '../lib/Subtabs.svelte';
  import MessageLog from '../lib/MessageLog.svelte';
  import Composer from '../lib/Composer.svelte';

  let newUser = '';
  let strip;
  let log;

  $: wanted = $route.items.chat;
  $: selected = wanted ? ($chatPartners.includes(wanted) ? wanted : null) : ($chatPartners[0] ?? null);
  $: items = $chatPartners.map((partner) => ({
    id: partner,
    label: partner,
    unread: $unread.has(chatKey(partner)),
  }));

  function openNew() {
    const username = newUser.trim();
    if (!username) return;
    openPrivateChat(username);
    newUser = '';
  }

  function close(username) {
    const next = strip.neighbour(username);
    if (selected === username) {
      if (next) go('chat', next);
      else forget('chat', username);
    }
    closeChat(username);
  }

  function closeAll() {
    const partners = [...$chatPartners];
    act(() => Promise.all(partners.map((user) => del(`/chats/${encodeURIComponent(user)}`))), {
      failed: 'Chats not closed',
      then: () => partners.forEach(forgetChat),
      done: `Closed ${plural(partners.length, 'chat')}`,
      action: { label: 'Undo', run: () => partners.forEach(openChat) },
    });
  }

  function tabMenu(event, partner) {
    openMenu(event, [
      ...userMenu(partner, { current: 'chat' }),
      { sep: true },
      { label: 'Close All Chats', action: closeAll },
      { sep: true },
      { label: 'Close Chat', action: () => close(partner) },
    ]);
  }
</script>

<div class="toolbar">
  <input {...noAutofill}
    placeholder="Start chat with…"
    bind:value={newUser}
    on:keydown={onEnter(openNew)}
  />
  <button on:click={openNew}>Open Chat</button>
</div>

<Subtabs
  tab="chat"
  {items}
  active={selected}
  bind:this={strip}
  onselect={(user) => go('chat', user)}
  onclose={close}
  onmenu={tabMenu}
/>

{#if selected}
  <MessageLog
    key={chatKey(selected)}
    messages={$conversations.get(chatKey(selected)) ?? []}
    self={$status.username}
    active={$route.tab === 'chat'}
    bind:this={log}
  />
  <Composer key={chatKey(selected)} placeholder="Message {selected}…" onsent={() => log.toBottom()} />
{:else}
  <p class="hint">No open chats.</p>
{/if}
