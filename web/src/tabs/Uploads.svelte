<script>
  import { uploads, isCleared } from '../lib/stores.js';
  import { post, formatSize, formatQuality, baseName } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';

  $: list = Object.values($uploads).sort((a, b) => b.updated_at - a.updated_at);

  function abortUserUploads(username) {
    for (const t of list) {
      if (t.username === username && !isCleared(t.status)) {
        post('/uploads/abort', { username: t.username, virtual_path: t.virtual_path });
      }
    }
  }

  function rowMenu(event, t) {
    const items = [];
    if (!isCleared(t.status)) {
      items.push(
        {
          label: 'Abort',
          action: () =>
            post('/uploads/abort', { username: t.username, virtual_path: t.virtual_path }),
        },
        { label: "Abort User's Uploads", action: () => abortUserUploads(t.username) },
        { sep: true },
      );
    }
    items.push(
      { label: 'Copy File Path', action: () => navigator.clipboard.writeText(t.virtual_path) },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(t.username) },
    );
    openMenu(event, items);
  }
</script>

<div class="toolbar">
  <button on:click={() => post('/uploads/clear', { statuses: ['finished'] })}>
    Clear Finished
  </button>
  <button on:click={() => post('/uploads/clear', { statuses: ['aborted', 'failed'] })}>
    Clear Inactive
  </button>
  <button
    on:click={() =>
      confirm('Clear all uploads? Active uploads will be aborted.') &&
      post('/uploads/clear_all')}
  >
    Clear All…
  </button>
  <span>{list.length} uploads</span>
</div>

<div class="scroll" tabindex="0">
  <table>
    <thead>
      <tr>
        <th>User</th>
        <th class="grow">Filename</th>
        <th>Status</th>
        <th>Progress</th>
        <th>Size</th>
        <th>Quality</th>
        <th>Speed</th>
      </tr>
    </thead>
    <tbody>
      {#each list as t (t.username + t.virtual_path)}
        <tr on:contextmenu={(e) => rowMenu(e, t)}>
          <td>{t.username}</td>
          <td class="grow" title={t.virtual_path}>{baseName(t.virtual_path)}</td>
          <td>{t.status}</td>
          <td>{t.size ? Math.floor((t.bytes_done / t.size) * 100) : 0}%</td>
          <td>{formatSize(t.size)}</td>
          <td>{formatQuality(t.attributes)}</td>
          <td>{t.speed_bps ? formatSize(t.speed_bps) + '/s' : ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
