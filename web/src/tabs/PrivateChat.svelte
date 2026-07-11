<script>
  import { privateChats, chatPartners, loadChatHistory, status } from '../lib/stores.js';
  import { activeTab, chatTarget } from '../lib/ui.js';
  import { post, del, formatTime } from '../lib/api.js';
  import { autoscroll } from '../lib/autoscroll.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';

  let selected = null;
  let newUser = '';
  let draft = '';
  let unread = {};
  let counts = {};
  const requested = new Set();

  $: messages = selected ? ($privateChats[selected] ?? []) : [];
  $: if (!$chatPartners.includes(selected) && $chatPartners.length) selected = $chatPartners[0];
  $: if (selected && !$privateChats[selected] && !requested.has(selected)) {
    requested.add(selected);
    loadChatHistory(selected);
  }

  $: {
    for (const [user, msgs] of Object.entries($privateChats)) {
      if (counts[user] !== undefined && msgs.length > counts[user]
          && !(user === selected && $activeTab === 'chat')) {
        unread[user] = true;
      }
      counts[user] = msgs.length;
    }
  }
  $: if ($activeTab === 'chat' && selected && unread[selected]) {
    delete unread[selected];
    unread = unread;
  }

  $: if ($chatTarget) {
    open($chatTarget);
    chatTarget.set(null);
  }

  function open(username) {
    selected = username;
    delete unread[username];
    unread = unread;
    if (!$chatPartners.includes(username)) {
      post(`/chats/${encodeURIComponent(username)}/open`);
      chatPartners.update((list) => [username, ...list]);
    }
  }

  function openNew() {
    if (!newUser.trim()) return;
    open(newUser.trim());
    newUser = '';
  }

  function close(username) {
    del(`/chats/${encodeURIComponent(username)}`);
    chatPartners.update((list) => list.filter((user) => user !== username));
    if (selected === username) selected = null;
  }

  function closeAll() {
    if (!confirm('Close all chat tabs?')) return;
    for (const partner of $chatPartners) close(partner);
  }

  function tabMenu(event, partner) {
    openMenu(event, [
      ...userMenu(partner, { skip: ['privatechat'] }),
      { sep: true },
      { label: 'Close All Tabs…', action: closeAll },
      { label: 'Close Tab', action: () => close(partner) },
    ]);
  }

  function send() {
    if (!draft.trim() || !selected) return;
    post(`/chats/${encodeURIComponent(selected)}`, { message: draft });
    draft = '';
  }
</script>

<div class="toolbar">
  <input
    placeholder="Start chat with…"
    bind:value={newUser}
    on:keydown={(e) => e.key === 'Enter' && openNew()}
  />
  <button on:click={openNew}>Open</button>
</div>

{#if $chatPartners.length}
  <div class="subtabs">
    {#each $chatPartners as partner}
      <span class="subtab">
        <button
          class:active={selected === partner}
          class:unread={unread[partner]}
          on:click={() => open(partner)}
          on:contextmenu={(e) => tabMenu(e, partner)}
        >
          {partner}
        </button>
        <button on:click={() => close(partner)}>Close</button>
      </span>
    {/each}
  </div>
{/if}

{#if selected}
  <div class="messages" tabindex="0" use:autoscroll={messages}>
    {#each messages as message}
      <div class="message">
        <span class="time">{formatTime(message.timestamp)}</span>
        <span class="sender" class:self={message.sender === $status.username}>
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
{:else}
  <span>Open a chat to start messaging.</span>
{/if}
