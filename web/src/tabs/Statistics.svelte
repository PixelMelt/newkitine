<script>
  import { get, post } from '../lib/api.js';
  import { loadIpBans } from '../lib/stores.js';
  import { act } from '../lib/act.js';
  import { persisted } from '../lib/persisted.js';
  import {
    formatSize, formatSpeed, formatCount, formatDateTime, baseName, plural,
  } from '../lib/format.js';
  import { route, openUserInfo } from '../lib/ui.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';

  const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
  const VIEWS = [
    ['overview', 'Overview'],
    ['files', 'Top Uploaded Files'],
    ['recipients', 'Top Recipients'],
    ['verdicts', 'Peer Verdicts'],
    ['peers', 'Peers Seen'],
  ];
  const COUNTRY_ROWS = 10;

  const view = persisted('statistics.view', 'overview', (v) => VIEWS.some(([id]) => id === v));
  const count = (key, label, width = 84) => ({
    key, label, width, align: 'end', desc: true,
    text: (r) => (r[key] === null ? '' : formatCount(r[key])), sortValue: (r) => r[key] ?? -1,
  });
  const when = (key, label) => ({
    key, label, width: 150, desc: true,
    text: (r) => (r[key] ? formatDateTime(r[key]) : ''), sortValue: (r) => r[key] ?? 0,
  });
  const user = { key: 'username', label: 'User', grow: 1, min: 140, text: (r) => r.username };
  const text = (key, label, width) => ({ key, label, width, text: (r) => r[key] ?? '' });

  const fileColumns = [
    { key: 'file', label: 'File', grow: 1, min: 240, tail: 12, text: (f) => baseName(f.virtual_path) },
    count('count', 'Times'),
    { key: 'bytes', label: 'Sent', width: 96, align: 'end', desc: true, text: (f) => formatSize(f.bytes), sortValue: (f) => f.bytes },
  ];
  const recipientColumns = [
    user,
    count('count', 'Files'),
    { key: 'bytes', label: 'Sent', width: 96, align: 'end', desc: true, text: (u) => formatSize(u.bytes), sortValue: (u) => u.bytes },
  ];
  const verdictColumns = [
    user,
    text('verdict', 'Verdict', 90),
    { key: 'evidence', label: 'Evidence', grow: 1, min: 160, text: (r) => String(r.evidence) },
    text('restriction', 'Restriction', 110),
    text('country', 'Country', 70),
    text('last_ip', 'IP Address', 120),
    count('shared_files', 'Shared Files', 100),
    when('convicted_at', 'Convicted'),
    when('last_seen', 'Last Seen'),
  ];
  const peerColumns = [
    user,
    text('country', 'Country', 70),
    count('searches', 'Searches'),
    count('searches_matched', 'Matched'),
    count('queue_requests', 'Queued'),
    count('queue_rejected', 'Rejected'),
    count('browses', 'Browses'),
    count('shared_files', 'Shared Files', 100),
    when('first_seen', 'First Seen'),
    when('last_seen', 'Last Seen'),
  ];

  let transfers = null;
  let peers = null;
  let verdicts = null;
  let failure = '';
  let loading = 0;

  function load(path, what, store) {
    loading += 1;
    return act(() => get(path), {
      failed: `${what} not loaded`,
      where: (message) => (failure = message),
      then: store,
    }).then(() => (loading -= 1));
  }

  function refresh() {
    if (loading) return;
    failure = '';
    load(`/stats/transfers?utc_offset=${-new Date().getTimezoneOffset() * 60}`, 'Transfer statistics', (data) => (transfers = data));
    load('/stats/peers?limit=1000', 'Peer statistics', (data) => (peers = data));
    load('/stats/verdicts', 'Peer verdicts', (data) => (verdicts = data.users));
  }

  $: if ($route.tab === 'statistics') refresh();
  $: waiting = failure ? '' : 'Loading statistics…';
  $: maxWeekday = transfers ? Math.max(1, ...transfers.weekday_uploads) : 1;
  $: countries = peers ? countryRows(peers.countries) : [];

  function countryRows(all) {
    const total = all.reduce((sum, entry) => sum + entry.count, 0);
    const top = all.slice(0, COUNTRY_ROWS).map((entry) => ({ label: entry.country, count: entry.count }));
    const other = all.slice(COUNTRY_ROWS).reduce((sum, entry) => sum + entry.count, 0);
    if (other) top.push({ label: 'Other', count: other });
    const largest = Math.max(1, ...top.map((entry) => entry.count));
    return top.map((entry) => ({
      ...entry,
      width: (entry.count / largest) * 100,
      percent: Math.round((entry.count / total) * 100),
    }));
  }

  function verdictMenu(rows, point) {
    const row = rows[0];
    openMenuAt(point, [
      { label: 'View User Profile', primary: true, action: () => openUserInfo(row.username) },
      {
        label: 'Clear Verdict',
        action: () =>
          act(() => post(`/users/${encodeURIComponent(row.username)}/clear_verdict`, {}), {
            failed: `Verdict on ${row.username} not cleared`,
            done: `Cleared the verdict on ${row.username}`,
            then: () => load('/stats/verdicts', 'Peer verdicts', (data) => (verdicts = data.users)),
          }),
      },
      {
        label: 'Ban Last IP Address', disabled: !row.last_ip,
        action: () =>
          act(() => post('/ip_bans', { pattern: row.last_ip }), {
            failed: `${row.last_ip} not banned`,
            done: `Banned IP address ${row.last_ip}`,
            then: loadIpBans,
          }),
      },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(row.username) },
    ]);
  }

  const userRowMenu = (rows, point) => openMenuAt(point, userMenu(rows[0].username, { primary: true }));
  const openProfile = (rows) => openUserInfo(rows[0].username);
</script>

<div class="toolbar">
  {#each VIEWS as [id, label]}
    <button class:active={$view === id} aria-pressed={$view === id} on:click={() => view.set(id)}>
      {label}
    </button>
  {/each}
  <span class="count">{loading ? (transfers ? 'Refreshing…' : '') : ''}</span>
  <button class="spaced" on:click={refresh}>Refresh</button>
  {#if failure}<span class="notice">{failure}</span>{/if}
</div>

{#if $view === 'overview'}
  {#if transfers && peers}
  <div class="scroll page">
    <h3>Totals</h3>
    <dl class="facts wide">
      <dt>Uploads finished</dt>
      <dd>{formatCount(transfers.totals.upload.count)}</dd>
      <dt>Uploaded</dt>
      <dd>{formatSize(transfers.totals.upload.bytes)}</dd>
      <dt>Average upload speed</dt>
      <dd>{formatSpeed(transfers.totals.upload.avg_speed_bps)}</dd>
      <dt>Uploaded to</dt>
      <dd>{plural(transfers.unique_upload_users, 'distinct user')}</dd>
      <dt>Downloads finished</dt>
      <dd>{formatCount(transfers.totals.download.count)}</dd>
      <dt>Downloaded</dt>
      <dd>{formatSize(transfers.totals.download.bytes)}</dd>
      <dt>Peers seen</dt>
      <dd>{formatCount(peers.total)}</dd>
    </dl>

    <h3>Uploads by Weekday</h3>
    <div class="bars">
      {#each transfers.weekday_uploads as uploads, day}
        <span>{WEEKDAYS[day]}</span>
        <div class="meter"><div class="fill" style:width="{(uploads / maxWeekday) * 100}%"></div></div>
        <span class="num">{formatCount(uploads)}</span>
        <span></span>
      {/each}
    </div>

    <h3>Peers by Country</h3>
    <div class="bars">
      {#each countries as entry}
        <span>{entry.label}</span>
        <div class="meter"><div class="fill" style:width="{entry.width}%"></div></div>
        <span class="num">{formatCount(entry.count)}</span>
        <span class="num dim">{entry.percent}%</span>
      {:else}
        <span class="hint">No peers seen yet.</span>
      {/each}
    </div>
  </div>
  {:else}
    <p class="hint">{waiting}</p>
  {/if}
{:else if $view === 'files'}
  <DataTable
    id="stats-files" label="Top uploaded files" rows={transfers ? transfers.top_files : []} loading={!transfers}
    key={(f) => f.virtual_path} columns={fileColumns} primary
    copyText={(f) => f.virtual_path} empty="Nothing has been uploaded yet."
  />
{:else if $view === 'recipients'}
  <DataTable
    id="stats-recipients" label="Top upload recipients" rows={transfers ? transfers.top_users : []} loading={!transfers}
    key={(u) => u.username} columns={recipientColumns} primary typeahead={(u) => u.username}
    copyText={(u) => u.username} empty="Nothing has been uploaded yet."
    onactivate={openProfile} onmenu={userRowMenu}
  />
{:else if $view === 'verdicts'}
  <p class="hint">
    Peers judged abusive or leech by client filtering. Clearing a verdict lifts the restriction and
    resets the peer's counters, so they are judged again from scratch. Adding one as a buddy
    exempts them permanently.
  </p>
  <DataTable
    id="stats-verdicts" label="Peer verdicts" rows={verdicts ?? []} loading={!verdicts}
    key={(u) => u.username} columns={verdictColumns} primary typeahead={(u) => u.username}
    copyText={(u) => u.username} empty="No peer has a verdict."
    onactivate={openProfile} onmenu={verdictMenu}
  />
{:else}
  <DataTable
    id="stats-peers" label="Peers seen" rows={peers ? peers.users : []} loading={!peers}
    key={(u) => u.username} columns={peerColumns} primary typeahead={(u) => u.username}
    copyText={(u) => u.username} empty="No peers seen yet."
    onactivate={openProfile} onmenu={userRowMenu}
  />
{/if}
