<script>
  import { downloads, isCleared } from '../lib/stores.js';
  import { post, formatSize, formatDuration, formatQuality, baseName } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';

  $: list = Object.values($downloads).sort((a, b) => b.updated_at - a.updated_at);

  const ref = (t) => ({ username: t.username, virtual_path: t.virtual_path });

  function rowMenu(event, t) {
    const items = [];
    if (isCleared(t.status) && t.status !== 'finished') {
      items.push({ label: 'Retry', action: () => post('/downloads/retry', ref(t)) });
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
        <th>User</th>
        <th class="grow">Filename</th>
        <th>Status</th>
        <th>Queue</th>
        <th>Progress</th>
        <th>Size</th>
        <th>Quality</th>
        <th>Speed</th>
        <th>Time Left</th>
      </tr>
    </thead>
    <tbody>
      {#each list as t (t.username + t.virtual_path)}
        <tr on:contextmenu={(e) => rowMenu(e, t)}>
          <td>{t.username}</td>
          <td class="grow" title={t.virtual_path}>{baseName(t.virtual_path)}</td>
          <td>{t.status}</td>
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
