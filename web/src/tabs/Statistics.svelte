<script>
  import { get, post } from '../lib/api.js';
  import { formatSize, formatTime, baseName } from '../lib/format.js';
  import { activeTab } from '../lib/ui.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

  let transfers = null;
  let peers = null;
  let verdicts = [];
  let fileSort = { key: null, dir: 1 };
  let userSort = { key: null, dir: 1 };
  let verdictSort = { key: null, dir: 1 };
  let peerSort = { key: null, dir: 1 };

  async function refresh() {
    let verdictsResponse;
    [transfers, peers, verdictsResponse] = await Promise.all([
      get('/stats/transfers'),
      get('/stats/peers'),
      get('/stats/verdicts'),
    ]);
    verdicts = verdictsResponse.users;
  }

  $: if ($activeTab === 'statistics') refresh();

  function formatDate(timestamp) {
    return new Date(timestamp * 1000).toLocaleDateString();
  }

  function rowMenu(event, username) {
    openMenu(event, [{ label: 'User Actions', submenu: userMenu(username) }]);
  }

  function verdictMenu(event, user) {
    const items = [];
    if (user.last_ip) {
      items.push({
        label: `Ban IP ${user.last_ip}`,
        action: () => post('/ip_bans', { pattern: user.last_ip }),
      });
      items.push({ sep: true });
    }
    items.push({ label: 'User Actions', submenu: userMenu(user.username) });
    openMenu(event, items);
  }

  $: uploads = transfers ? transfers.totals.upload : { count: 0, bytes: 0, avg_speed_bps: 0 };
  $: uploadUsers = transfers ? transfers.unique_upload_users : 0;
  $: downloads = transfers ? transfers.totals.download : { count: 0, bytes: 0 };
  $: maxWeekday = Math.max(1, ...(transfers ? transfers.weekday_uploads : [0]));
  $: pie = buildPie(peers ? peers.countries : []);
  $: topFiles = sortRows(transfers ? transfers.top_files : [], fileSort, {
    file: (f) => baseName(f.virtual_path),
  });
  $: topUsers = sortRows(transfers ? transfers.top_users : [], userSort);
  $: verdictRows = sortRows(verdicts, verdictSort);
  $: peerRows = sortRows(peers ? peers.users : [], peerSort);

  function buildPie(countries) {
    const total = countries.reduce((sum, entry) => sum + entry.count, 0);
    if (!total) return null;
    const slices = countries.slice(0, 8).map((entry) => ({
      label: entry.country,
      count: entry.count,
    }));
    const other = countries.slice(8).reduce((sum, entry) => sum + entry.count, 0);
    if (other) slices.push({ label: 'Other', count: other });
    let angle = 0;
    const stops = [];
    for (const [index, slice] of slices.entries()) {
      slice.color = `hsl(${(index * 137) % 360} 40% 50%)`;
      slice.percent = Math.round((slice.count / total) * 100);
      const start = angle;
      angle += (slice.count / total) * 360;
      stops.push(`${slice.color} ${start}deg ${angle}deg`);
    }
    return { slices, gradient: `conic-gradient(${stops.join(', ')})` };
  }
</script>

<div class="toolbar">
  <button on:click={refresh}>Refresh</button>
  <span>{peers ? peers.total : 0} peers seen</span>
  <span>uploaded to {uploadUsers} distinct {uploadUsers === 1 ? 'user' : 'users'}</span>
</div>

<div class="scroll stats" tabindex="0">
  <h3>Totals</h3>
  <table>
    <thead>
      <tr>
        <th class="grow">Direction</th>
        <th>Finished</th>
        <th>Total Size</th>
        <th>Average Speed</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td class="grow">Uploads</td>
        <td>{uploads.count}</td>
        <td>{formatSize(uploads.bytes)}</td>
        <td>{formatSize(uploads.avg_speed_bps)}/s</td>
      </tr>
      <tr>
        <td class="grow">Downloads</td>
        <td>{downloads.count}</td>
        <td>{formatSize(downloads.bytes)}</td>
        <td></td>
      </tr>
    </tbody>
  </table>

  <h3>Top Uploaded Files</h3>
  <table>
    <thead>
      <tr>
        <Th bind:sort={fileSort} key="file" grow>File</Th>
        <Th bind:sort={fileSort} key="count">Times</Th>
        <Th bind:sort={fileSort} key="bytes">Sent</Th>
      </tr>
    </thead>
    <tbody>
      {#each topFiles as file}
        <tr>
          <td class="grow" title={file.virtual_path}>{baseName(file.virtual_path)}</td>
          <td>{file.count}</td>
          <td>{formatSize(file.bytes)}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h3>Top Upload Recipients</h3>
  <table>
    <thead>
      <tr>
        <Th bind:sort={userSort} key="username" grow>User</Th>
        <Th bind:sort={userSort} key="count">Files</Th>
        <Th bind:sort={userSort} key="bytes">Sent</Th>
      </tr>
    </thead>
    <tbody>
      {#each topUsers as user}
        <tr on:contextmenu={(e) => rowMenu(e, user.username)}>
          <td class="grow">{user.username}</td>
          <td>{user.count}</td>
          <td>{formatSize(user.bytes)}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h3>Uploads by Weekday</h3>
  <div class="bars">
    {#each transfers ? transfers.weekday_uploads : [] as count, day}
      <div class="bar-row">
        <span class="bar-label">{WEEKDAYS[day]}</span>
        <div class="bar-track">
          <div class="bar" style:width="{(count / maxWeekday) * 100}%"></div>
        </div>
        <span class="bar-count">{count}</span>
      </div>
    {/each}
  </div>

  {#if pie}
    <h3>Peer Countries</h3>
    <div class="pie-section">
      <div class="pie" style:background={pie.gradient}></div>
      <div class="pie-legend">
        {#each pie.slices as slice}
          <div class="pie-entry">
            <span class="swatch" style:background={slice.color}></span>
            <span>{slice.label}: {slice.count} ({slice.percent}%)</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}

  {#if verdicts.length}
    <h3>Peer Verdicts</h3>
    <p class="hint">
      Peers judged suspect or leech by client filtering. Adding one as a buddy clears the
      verdict and lifts any restriction.
    </p>
    <div class="table-scroll">
    <table>
      <thead>
        <tr>
          <Th bind:sort={verdictSort} key="username" grow>User</Th>
          <Th bind:sort={verdictSort} key="verdict">Verdict</Th>
          <Th bind:sort={verdictSort} key="evidence">Evidence</Th>
          <Th bind:sort={verdictSort} key="restriction">Restriction</Th>
          <Th bind:sort={verdictSort} key="country">Country</Th>
          <Th bind:sort={verdictSort} key="last_ip">IP</Th>
          <Th bind:sort={verdictSort} key="shared_files">Shared Files</Th>
          <Th bind:sort={verdictSort} key="last_seen">Last Seen</Th>
        </tr>
      </thead>
      <tbody>
        {#each verdictRows as user (user.username)}
          <tr on:contextmenu={(e) => verdictMenu(e, user)}>
            <td class="grow">{user.username}</td>
            <td>{user.verdict}</td>
            <td>{user.evidence}</td>
            <td>{user.restriction}</td>
            <td>{user.country ?? ''}</td>
            <td>{user.last_ip ?? ''}</td>
            <td>{user.shared_files ?? ''}</td>
            <td>{formatDate(user.last_seen)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    </div>
  {/if}

  <h3>Peers Seen</h3>
  <div class="table-scroll">
  <table>
    <thead>
      <tr>
        <Th bind:sort={peerSort} key="username" grow>User</Th>
        <Th bind:sort={peerSort} key="country">Country</Th>
        <Th bind:sort={peerSort} key="searches">Searches</Th>
        <Th bind:sort={peerSort} key="searches_matched">Matched</Th>
        <Th bind:sort={peerSort} key="queue_requests">Queued</Th>
        <Th bind:sort={peerSort} key="queue_rejected">Rejected</Th>
        <Th bind:sort={peerSort} key="browses">Browses</Th>
        <Th bind:sort={peerSort} key="shared_files">Shared Files</Th>
        <Th bind:sort={peerSort} key="first_seen">First Seen</Th>
        <Th bind:sort={peerSort} key="last_seen">Last Seen</Th>
      </tr>
    </thead>
    <tbody>
      {#each peerRows as user (user.username)}
        <tr on:contextmenu={(e) => rowMenu(e, user.username)}>
          <td class="grow">{user.username}</td>
          <td>{user.country ?? ''}</td>
          <td>{user.searches}</td>
          <td>{user.searches_matched}</td>
          <td>{user.queue_requests}</td>
          <td>{user.queue_rejected}</td>
          <td>{user.browses}</td>
          <td>{user.shared_files ?? ''}</td>
          <td>{formatDate(user.first_seen)}</td>
          <td>{formatDate(user.last_seen)} {formatTime(user.last_seen)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  </div>
</div>

<style>
  .stats h3 {
    margin: 12px 8px 4px;
    font-size: 13px;
  }
  .stats p.hint {
    margin: 0 8px 4px;
  }
  .stats table {
    width: calc(100% - 16px);
    margin: 0 8px 8px;
  }
  .stats :global(th) {
    position: static;
  }
  .table-scroll {
    max-height: 320px;
    overflow: auto;
    margin: 0 8px 8px;
    border: 1px solid var(--border-soft);
  }
  .table-scroll table {
    width: 100%;
    margin: 0;
  }
  .table-scroll :global(th) {
    position: sticky;
    top: 0;
  }
  .bars {
    margin: 0 8px 8px;
    max-width: 480px;
  }
  .bar-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .bar-label {
    width: 80px;
  }
  .bar-track {
    flex: 1;
    height: 12px;
    background: var(--chrome);
    border: 1px solid var(--border-faint);
  }
  .bar {
    height: 100%;
    background: var(--selected);
  }
  .bar-count {
    width: 48px;
    text-align: right;
  }
  .pie-section {
    display: flex;
    align-items: center;
    gap: 24px;
    margin: 4px 8px 8px;
  }
  .pie {
    width: 140px;
    height: 140px;
    border-radius: 50%;
    border: 1px solid var(--border-faint);
    flex-shrink: 0;
  }
  .pie-legend {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pie-entry {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border: 1px solid var(--border-faint);
    flex-shrink: 0;
  }
</style>
