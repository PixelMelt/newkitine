<script>
  import { tick } from 'svelte';
  import { downloads, uploads, isCleared, report } from '../lib/stores.js';
  import { openBrowse, openUserInfo } from '../lib/ui.js';
  import { post } from '../lib/api.js';
  import { act, actEach, copy } from '../lib/act.js';
  import {
    formatSize, formatSpeed, formatDuration, formatQuality, formatCount,
    effectiveBitrate, baseName, folderName, plural,
  } from '../lib/format.js';
  import { describe, toneRank, retriesByItself } from '../lib/transferStatus.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';

  export let direction;

  const isDownload = direction === 'download';
  const path = isDownload ? '/downloads' : '/uploads';
  const noun = isDownload ? 'download' : 'upload';
  const store = isDownload ? downloads : uploads;
  const progress = (t) => (t.size ? t.bytes_done / t.size : 0);
  const secondsLeft = (t) => (t.size - t.bytes_done) / t.speed_bps;
  const status = (t) => describe(t, direction);
  const TONE_CLASS = { failed: 'bad', waiting: 'dim', stopped: 'dim', done: 'dim', active: '' };
  const CONFIRM_MS = 6000;

  const columns = [
    { key: 'username', label: 'User', width: 140, text: (t) => t.username },
    {
      key: 'filename', label: 'Filename', grow: 3, min: 220, tail: 12,
      text: (t) => baseName(t.virtual_path),
    },
    {
      key: 'status', label: 'Status', grow: 2, min: 150,
      text: (t) => status(t).text, sortValue: (t) => toneRank(status(t).tone),
      class: (t) => TONE_CLASS[status(t).tone], hint: (t) => status(t).hint,
    },
    ...(isDownload
      ? [{
          key: 'queue_place', label: 'Queue', width: 62, align: 'end',
          text: (t) => (t.queue_place ? String(t.queue_place) : ''),
          sortValue: (t) => t.queue_place || Number.MAX_SAFE_INTEGER,
        }]
      : []),
    { key: 'progress', label: 'Progress', width: 130, progress, sortValue: progress, desc: true },
    {
      key: 'size', label: 'Size', width: 84, align: 'end', desc: true,
      text: (t) => formatSize(t.size), sortValue: (t) => t.size,
    },
    {
      key: 'quality', label: 'Quality', width: 124, desc: true,
      text: (t) => formatQuality(t.attributes), sortValue: (t) => effectiveBitrate(t.attributes),
    },
    {
      key: 'speed', label: 'Speed', width: 92, align: 'end', desc: true,
      text: (t) => (t.speed_bps ? formatSpeed(t.speed_bps) : ''), sortValue: (t) => t.speed_bps,
    },
    ...(isDownload
      ? [{
          key: 'time_left', label: 'Time Left', width: 82, align: 'end',
          text: (t) => (t.speed_bps ? formatDuration(secondsLeft(t)) : ''),
          sortValue: (t) => (t.speed_bps ? secondsLeft(t) : Number.MAX_SAFE_INTEGER),
        }]
      : []),
  ];

  let selection = new Set();

  const live = (t) =>
    !isCleared(t.status) || (isDownload && retriesByItself(t));

  const running = (t) => (t.status === 'transferring' ? 0 : 1);

  $: all = Object.values($store).sort((a, b) => running(a) - running(b) || b.id - a.id);
  $: selected = all.filter((t) => selection.has(t.id));
  $: activeCount = all.filter(live).length;
  $: finishedCount = all.filter((t) => t.status === 'finished').length;
  $: inactiveCount = all.filter((t) => t.status === 'aborted' || t.status === 'failed').length;

  let confirming = false;
  let confirmTimer = null;
  let cancelButton;

  const abortable = (rows) => rows.filter(live);
  const retryable = (rows) => rows.filter((t) => t.status === 'aborted' || t.status === 'failed');
  const nameOf = (rows) =>
    rows.length === 1 ? `"${baseName(rows[0].virtual_path)}"` : plural(rows.length, noun);

  async function retry(rows) {
    const targets = retryable(rows);
    if (!targets.length) return;
    await actEach(
      targets.map((t) => post('/downloads/retry', { id: t.id })),
      { failed: (count) => `${count === targets.length ? nameOf(targets) : plural(count, noun)} not retried`, skip: [409] },
    );
  }

  async function abort(rows) {
    const targets = abortable(rows);
    if (!targets.length) return;
    const result = await actEach(
      targets.map((t) => post(`${path}/abort`, { id: t.id })),
      { failed: (count) => `${count === targets.length ? nameOf(targets) : plural(count, noun)} not stopped` },
    );
    if (result.ok) {
      report.done(
        `Stopped ${nameOf(targets)}`,
        isDownload ? { label: 'Retry', run: () => retry(targets.map((t) => $store[t.id]).filter(Boolean)) } : undefined,
      );
    }
  }

  const uploadsTo = (username) => abortable(all.filter((t) => t.username === username));

  function activate(rows) {
    if (isDownload) openBrowse(rows[0].username, folderName(rows[0].virtual_path));
    else openUserInfo(rows[0].username);
  }

  function rowMenu(rows, point) {
    const aborts = abortable(rows).length;
    const retries = retryable(rows).length;
    const many = rows.length > 1;
    const sameUser = rows.every((t) => t.username === rows[0].username);
    const items = [];
    if (isDownload) {
      items.push({ label: 'Browse Folder', primary: true, disabled: many, action: () => activate(rows) });
      items.push({ sep: true });
      items.push({
        label: many ? `Retry ${retries}` : 'Retry', disabled: !retries, action: () => retry(rows),
      });
    }
    items.push(
      {
        label: many ? `Stop ${aborts}` : 'Stop', disabled: !aborts,
        action: () => abort(rows),
      },
      ...(isDownload
        ? []
        : [{
            label: `Stop All Uploads to ${rows[0].username}`,
            disabled: !sameUser || !uploadsTo(rows[0].username).length,
            action: () => abort(uploadsTo(rows[0].username)),
          }]),
      { sep: true },
      {
        label: many ? `Copy ${rows.length} File Paths` : 'Copy File Path',
        action: () =>
          copy(rows.map((t) => t.virtual_path).join('\n'), many ? 'file paths' : 'file path'),
      },
      { sep: true },
      { label: 'User Actions', disabled: !sameUser, submenu: userMenu(rows[0].username) },
    );
    openMenuAt(point, items);
  }

  function clear(statuses, count, what) {
    if (!count) return;
    act(() => post(`${path}/clear`, { statuses }), {
      failed: `${what} not cleared`,
      done: `Cleared ${plural(count, `${what} ${noun}`)}`,
    });
  }

  function clearAll() {
    clearTimeout(confirmTimer);
    confirming = false;
    if (!all.length) return;
    const total = all.length;
    act(() => post(`${path}/clear_all`), {
      failed: `${isDownload ? 'Downloads' : 'Uploads'} not cleared`,
      done: `Cleared ${plural(total, noun)}`,
    });
  }

  async function askClearAll() {
    if (!activeCount) return clearAll();
    confirming = true;
    confirmTimer = setTimeout(() => (confirming = false), CONFIRM_MS);
    await tick();
    cancelButton.focus();
  }

  function cancelClear() {
    clearTimeout(confirmTimer);
    confirming = false;
  }
</script>

<div class="toolbar">
  {#if confirming}
    <button class="danger" on:click={clearAll}>
      Stop {plural(activeCount, `active ${noun}`)} and clear all {formatCount(all.length)}
    </button>
    <button
      bind:this={cancelButton}
      on:click={cancelClear}
      on:keydown={(e) => e.key === 'Escape' && cancelClear()}
    >
      Cancel
    </button>
  {:else}
    <button on:click={() => clear(['finished'], finishedCount, 'finished')}>Clear Finished</button>
    <button on:click={() => clear(['aborted', 'failed'], inactiveCount, 'failed or stopped')}>
      Clear Inactive
    </button>
    <button on:click={askClearAll}>{activeCount ? 'Clear All…' : 'Clear All'}</button>
  {/if}
  <span class="count">
    {#if selected.length > 1}
      {formatCount(selected.length)} of {plural(all.length, noun)} selected ·
      {formatSize(selected.reduce((sum, t) => sum + t.size, 0))}
    {:else}
      {plural(all.length, noun)}
    {/if}
  </span>
</div>

<DataTable
  id={noun + 's'}
  label={isDownload ? 'Downloads' : 'Uploads'}
  rows={all}
  key={(t) => t.id}
  {columns}
  primary
  bind:selection
  empty={`No ${noun}s.`}
  copyText={(t) => t.virtual_path}
  onactivate={activate}
  onmenu={rowMenu}
  ondelete={isDownload ? abort : null}
/>
