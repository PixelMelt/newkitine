<script>
  import { downloads, isCleared, notice } from '../lib/stores.js';
  import { post, formatSize, formatDuration, formatQuality, baseName } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  let sort = { key: null, dir: 1 };

  $: list = sortRows(
    Object.values($downloads).sort((a, b) => b.updated_at - a.updated_at),
    sort,
    {
      filename: (t) => baseName(t.virtual_path),
      progress: (t) => (t.size ? t.bytes_done / t.size : 0),
      quality: (t) => formatQuality(t.attributes),
      speed: (t) => t.speed_bps,
      time_left: (t) =>
        t.speed_bps ? (t.size - t.bytes_done) / t.speed_bps : Number.MAX_VALUE,
    },
  );

  const ref = (t) => ({ id: t.id });

  function rowMenu(event, t) {
    const items = [];
    if (isCleared(t.status) && t.status !== 'finished') {
      items.push({
        label: 'Retry',
        action: () =>
          post('/downloads/retry', ref(t)).catch((error) =>
            notice(error.status === 409 ? 'download already active' : error.message),
          ),
      });
    } else if (t.status !== 'finished') {
      items.push({ label: 'Abort', action: () => post('/downloads/abort', ref(t)) });
    }
    if (items.length) items.push({ sep: true });
    items.push(
      { label: 'Copy File Path', action: () => navigator.clipboard.writeText(t.virtual_path) },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(t.username) },
    );
    openMenu(event, items);
  }
</script>

<div class="toolbar">
  <button on:click={() => post('/downloads/clear', { statuses: ['finished'] })}>
    Clear Finished
  </button>
  <button on:click={() => post('/downloads/clear', { statuses: ['aborted', 'failed'] })}>
    Clear Inactive
  </button>
  <button
    on:click={() =>
      confirm('Clear all downloads? Active downloads will be aborted.') &&
      post('/downloads/clear_all')}
  >
    Clear All…
  </button>
  <span>{list.length} downloads</span>
</div>

<div class="scroll" tabindex="0">
  <table>
    <thead>
      <tr>
        <Th bind:sort key="username">User</Th>
        <Th bind:sort key="filename" grow>Filename</Th>
        <Th bind:sort key="status">Status</Th>
        <Th bind:sort key="queue_place">Queue</Th>
        <Th bind:sort key="progress">Progress</Th>
        <Th bind:sort key="size">Size</Th>
        <Th bind:sort key="quality">Quality</Th>
        <Th bind:sort key="speed">Speed</Th>
        <Th bind:sort key="time_left">Time Left</Th>
      </tr>
    </thead>
    <tbody>
      {#each list as t (t.id)}
        <tr on:contextmenu={(e) => rowMenu(e, t)}>
          <td>{t.username}</td>
          <td class="grow" title={t.virtual_path}>{baseName(t.virtual_path)}</td>
          <td>{t.failure_reason ? `${t.status}: ${t.failure_reason}` : t.status}</td>
          <td>{t.queue_place || ''}</td>
          <td>
            <progress max={t.size || 1} value={t.bytes_done}></progress>
            {t.size ? Math.floor((t.bytes_done / t.size) * 100) : 0}%
          </td>
          <td>{formatSize(t.size)}</td>
          <td>{formatQuality(t.attributes)}</td>
          <td>{t.speed_bps ? formatSize(t.speed_bps) + '/s' : ''}</td>
          <td>{t.speed_bps ? formatDuration((t.size - t.bytes_done) / t.speed_bps) : ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
