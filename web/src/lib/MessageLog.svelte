<script>
  import { afterUpdate, onMount } from 'svelte';
  import { markRead } from './stores.js';
  import { formatTime, formatDay, formatDateTime, segments, plural } from './format.js';
  import { openBrowse, decodeSegment } from './ui.js';

  export let key;
  export let messages;
  export let self;
  export let active;
  export let senderMenu = null;

  const PIN_SLACK = 8;
  const views = new Map();

  let node;
  let pinned = true;
  let seenId = 0;
  let shownKey = key;
  let restoreTop = null;

  $: if (key !== shownKey) {
    views.set(shownKey, { pinned, seenId, scrollTop: node.scrollTop });
    const view = views.get(key) ?? { pinned: true, seenId: 0, scrollTop: 0 };
    shownKey = key;
    pinned = view.pinned;
    seenId = view.seenId;
    restoreTop = view.scrollTop;
  }
  $: lastId = messages.length ? messages.at(-1).id : 0;
  $: if (pinned) seenId = lastId;
  $: newCount = pinned ? 0 : messages.filter((m) => m.id > seenId && m.sender !== self).length;
  const dayNumber = (timestamp) => {
    const date = new Date(timestamp * 1000);
    return date.getFullYear() * 512 + date.getMonth() * 32 + date.getDate();
  };

  $: rows = messages.map((message, index) => ({
    message,
    day:
      index === 0 || dayNumber(messages[index - 1].timestamp) !== dayNumber(message.timestamp)
        ? formatDay(message.timestamp)
        : null,
    emote: message.message.startsWith('/me '),
  }));

  function read() {
    if (active && pinned && document.visibilityState === 'visible') markRead(key);
  }

  function scrolled() {
    if (!node.clientHeight) return;
    pinned = node.scrollHeight - node.scrollTop - node.clientHeight <= PIN_SLACK;
    read();
  }

  export function toBottom() {
    pinned = true;
    node.scrollTop = node.scrollHeight;
  }

  afterUpdate(() => {
    if (pinned) node.scrollTop = node.scrollHeight;
    else if (restoreTop !== null) node.scrollTop = restoreTop;
    restoreTop = null;
    read();
  });

  onMount(() => {
    const observer = new ResizeObserver(() => {
      if (pinned) node.scrollTop = node.scrollHeight;
      read();
    });
    observer.observe(node);
    document.addEventListener('visibilitychange', read);
    return () => {
      observer.disconnect();
      document.removeEventListener('visibilitychange', read);
    };
  });

  function follow(event, link) {
    if (!link.startsWith('slsk://')) return;
    event.preventDefault();
    const [user, ...folders] = link.slice(7).split('/').filter(Boolean).map(decodeSegment);
    if (user) openBrowse(user, folders.length ? folders.join('\\') : null);
  }

  function keydown(event) {
    if (event.key === 'End') toBottom();
  }
</script>

<div class="log">
  <div
    class="messages"
    role="log"
    tabindex="0"
    bind:this={node}
    on:scroll={scrolled}
    on:keydown={keydown}
  >
    {#each rows as { message, day, emote } (message.id)}
      {#if day}<div class="day">{day}</div>{/if}
      <div class="message" class:emote>
        <span class="time" title={formatDateTime(message.timestamp)}
          >{formatTime(message.timestamp)}</span
        >
        <span
          class="sender"
          class:self={message.sender === self}
          on:contextmenu={(e) => senderMenu && senderMenu(e, message.sender)}
          >{emote ? `* ${message.sender}` : `${message.sender}:`}</span
        >
        <span class="text"
          >{#each segments(emote ? message.message.slice(4) : message.message) as part}{#if part.link}<a
                href={part.text}
                target="_blank"
                rel="noopener noreferrer"
                on:click={(e) => follow(e, part.text)}>{part.text}</a
              >{:else}{part.text}{/if}{/each}</span
        >
      </div>
    {:else}
      <div class="hint">No messages yet.</div>
    {/each}
  </div>
  {#if newCount}
    <button class="jump" on:click={toBottom}>{plural(newCount, 'new message')} ↓</button>
  {/if}
</div>
