<script>
  import { tick } from 'svelte';
  import {
    browses, browseFailure, browseLoad, downloadIndex, transferKey, queueDownloads, link,
    report,
  } from '../lib/stores.js';
  import { route, go, openBrowse, noAutofill } from '../lib/ui.js';
  import { shortState } from '../lib/transferStatus.js';
  import { get, post } from '../lib/api.js';
  import { act, copy } from '../lib/act.js';
  import {
    formatSize, formatQuality, formatDuration, effectiveBitrate, plural,
  } from '../lib/format.js';
  import { onEnter } from '../lib/keys.js';
  import { openMenuAt } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import DataTable from '../lib/DataTable.svelte';
  import Tree from '../lib/Tree.svelte';

  const FOLDER_FILTER_LIMIT = 5000;

  let username = '';
  let viewing = null;
  let filter = '';
  let filterRun = 0;
  let selectRun = 0;
  let tree = Object.create(null);
  let expanded = Object.create(null);
  let treeLoaded = false;
  let flatFolders = null;
  let flatSelection = new Set();
  let selectedDir = null;
  let listing = null;
  let loadedAt = 0;
  let summary = null;
  let request = null;
  let fileSelection = new Set();
  let pendingFolder = null;

  const encode = encodeURIComponent;
  const stateOf = (index, file) =>
    index.get(transferKey(listing.username, `${listing.dir}\\${file.name}`));

  const makeColumns = (index) => [
    { key: 'name', label: 'Filename', grow: 1, min: 200, tail: 12, text: (f) => f.name },
    {
      key: 'size', label: 'Size', width: 84, align: 'end', desc: true,
      text: (f) => formatSize(f.size), sortValue: (f) => f.size,
    },
    {
      key: 'quality', label: 'Quality', width: 124, desc: true,
      text: (f) => formatQuality(f.attributes), sortValue: (f) => effectiveBitrate(f.attributes),
    },
    {
      key: 'length', label: 'Length', width: 64, align: 'end', desc: true,
      text: (f) => (f.attributes.length ? formatDuration(f.attributes.length) : ''),
      sortValue: (f) => f.attributes.length ?? 0,
    },
    {
      key: 'state', label: 'Download', width: 80,
      text: (f) => (stateOf(index, f) ? shortState(stateOf(index, f)) : ''), class: () => 'dim',
    },
  ];
  const folderColumns = [
    { key: 'directory', label: 'Folder', grow: 1, min: 160, tail: 24, text: (f) => f.directory },
    {
      key: 'file_count', label: 'Files', width: 60, align: 'end', desc: true,
      text: (f) => f.file_count.toLocaleString(), sortValue: (f) => f.file_count,
    },
  ];

  $: columns = makeColumns($downloadIndex);
  $: wanted = $route.items.browse ?? null;
  $: onBrowse = $route.tab === 'browse';
  $: if ($link.state === 'live' && onBrowse && wanted !== viewing) openUser(wanted, $route.folder);
  $: reloadSameSecond($browseLoad);
  $: if (viewing && $browses.has(viewing) && $browses.get(viewing) !== loadedAt) {
    loadedAt = $browses.get(viewing);
    loadTree(viewing, selectedDir ?? pendingFolder);
  }
  $: if (onBrowse && treeLoaded && wanted === viewing && $route.folder && $route.folder !== selectedDir) {
    const folder = $route.folder;
    tick().then(() => folder !== selectedDir && revealFolder(viewing, folder));
  }
  $: if (viewing && loadedAt && !$browses.has(viewing)) reset();
  $: failRequest($browseFailure);
  $: visible = flatten(tree, expanded, '', 0);
  function reset() {
    loadedAt = 0;
    tree = Object.create(null);
    expanded = Object.create(null);
    treeLoaded = false;
    flatFolders = null;
    selectedDir = null;
    listing = null;
    summary = null;
  }

  function flatten(tree, expanded, path, depth) {
    const out = [];
    for (const node of tree[path] ?? []) {
      out.push({ node, depth });
      if (expanded[node.path]) out.push(...flatten(tree, expanded, node.path, depth + 1));
    }
    return out;
  }

  async function loadChildren(user, dir) {
    const data = await get(`/users/${encode(user)}/tree?dir=${encode(dir)}`);
    if (user !== viewing) return false;
    tree[dir] = data.children;
    tree = tree;
    if (data.summary) summary = data.summary;
    return true;
  }

  async function loadTree(user, folder) {
    const reopen = Object.keys(expanded).filter((path) => expanded[path]);
    const picked = selectRun;
    tree = Object.create(null);
    if (!(await loadChildren(user, ''))) return;
    treeLoaded = true;
    request = null;
    for (const path of reopen) {
      if (user === viewing) await loadChildren(user, path);
    }
    if (folder && user === viewing && picked === selectRun) await revealFolder(user, folder, picked);
    if (flatFolders && user === viewing) await applyFilter();
  }

  async function revealFolder(user, folder, picked = selectRun) {
    const parts = folder.split('\\');
    let path = '';
    for (const part of parts.slice(0, -1)) {
      path = path ? `${path}\\${part}` : part;
      if (!tree[path] && !(await loadChildren(user, path))) return;
      if (picked !== selectRun) return;
      expanded[path] = true;
    }
    expanded = expanded;
    const parent = parts.slice(0, -1).join('\\');
    const node = (tree[parent] ?? []).find((child) => child.path === folder);
    if (node) return selectFolder(folder, node.file_count > 0);
    pendingFolder = null;
    if (folder === selectedDir) {
      selectedDir = null;
      listing = null;
      fileSelection = new Set();
    }
    if ($route.tab === 'browse') go('browse', user, selectedDir);
    report.error(`"${folder}" is not in the share list of ${user}`);
  }

  async function toggle(node) {
    expanded[node.path] = !expanded[node.path];
    expanded = expanded;
    if (expanded[node.path] && !tree[node.path]) await loadChildren(viewing, node.path);
  }

  async function selectFolder(dir, hasFiles) {
    const user = viewing;
    selectRun += 1;
    selectedDir = dir;
    fileSelection = new Set();
    pendingFolder = null;
    if ($route.tab === 'browse') go('browse', user, dir);
    if (!hasFiles) {
      listing = { username: user, dir, files: [] };
      return;
    }
    await act(() => get(`/users/${encode(user)}/files?dir=${encode(dir)}`), {
      failed: `Files in "${dir}" not loaded`,
      where: (text) => {
        report.error(text);
        if (user === viewing && dir === selectedDir) selectedDir = null;
      },
      then: (data) => {
        if (user === viewing && dir === selectedDir) listing = { username: user, dir, files: data.files };
      },
    });
  }

  function reloadSameSecond(load) {
    if (load?.username === viewing && loadedAt && $browses.get(viewing) === loadedAt) {
      loadTree(viewing, selectedDir ?? pendingFolder);
    }
  }

  function failRequest(failure) {
    if (failure?.username === viewing && request) {
      request = { failed: `${viewing} did not send a share list` };
    }
  }

  async function requestList(user) {
    browseFailure.set(null);
    request = { failed: null };
    await act(() => post(`/users/${encode(user)}/browse`), {
      failed: `Share list of ${user} not requested`,
      where: (text) => user === viewing && (request = { ...request, failed: text }),
    });
  }

  function openUser(target, folder) {
    viewing = target;
    pendingFolder = folder;
    filter = '';
    request = null;
    reset();
    if (!target) return;
    username = target;
    if ($browses.has(target)) {
      loadedAt = $browses.get(target);
      loadTree(target, folder);
    } else requestList(target);
  }

  function submit() {
    const target = username.trim();
    if (!target) return;
    if (target === viewing && !treeLoaded) requestList(target);
    else openBrowse(target);
  }

  async function applyFilter() {
    flatSelection = new Set();
    const run = ++filterRun;
    if (!filter.trim()) {
      flatFolders = null;
      return;
    }
    const user = viewing;
    const data = await get(
      `/users/${encode(user)}/folders?filter=${encode(filter.trim())}&limit=${FOLDER_FILTER_LIMIT}`,
    );
    if (user === viewing && run === filterRun) flatFolders = data;
  }

  function clearFilter() {
    filterRun += 1;
    filter = '';
    flatFolders = null;
  }

  function revealFromFilter(rows) {
    clearFilter();
    revealFolder(viewing, rows[0].directory);
  }

  const downloadFiles = (files) =>
    queueDownloads(
      files.map((file) => ({
        username: listing.username,
        virtual_path: `${listing.dir}\\${file.name}`,
        size: file.size,
        attributes: file.attributes,
      })),
    );

  function downloadFolder(dir, recursive) {
    act(() => post('/downloads/folder', { username: viewing, dir, recursive }), {
      failed: `Folder "${dir}" not queued`,
      done: ({ enqueued, already_active }) =>
        `Queued ${plural(enqueued, 'file')} from "${dir}"` +
        (already_active ? ` (${already_active} already in Downloads)` : ''),
      action: { label: 'Show Downloads', run: () => go('downloads') },
    });
  }

  function folderItems(dir) {
    return [
      { label: 'Download Folder', action: () => downloadFolder(dir, false) },
      { label: 'Download Folder and Subfolders', action: () => downloadFolder(dir, true) },
      { sep: true },
      { label: 'Copy Folder Path', action: () => copy(dir, 'folder path') },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(viewing, { current: 'browse' }) },
    ];
  }

  function fileMenu(files, point) {
    const many = files.length > 1;
    openMenuAt(point, [
      {
        label: many ? `Download ${files.length} Files` : 'Download File',
        primary: true, action: () => downloadFiles(files),
      },
      { label: 'Download Folder', action: () => downloadFolder(listing.dir, false) },
      { sep: true },
      {
        label: many ? `Copy ${files.length} File Paths` : 'Copy File Path',
        action: () =>
          copy(
            files.map((f) => `${listing.dir}\\${f.name}`).join('\n'),
            many ? 'file paths' : 'file path',
          ),
      },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(viewing, { current: 'browse' }) },
    ]);
  }
</script>

<div class="toolbar">
  <input {...noAutofill}
    data-primary={treeLoaded ? null : ''}
    placeholder="Browse shares of…"
    bind:value={username}
    on:focus={(e) => e.target.select()}
    on:keydown={onEnter(submit)}
  />
  <button on:click={submit}>Browse</button>
  {#if treeLoaded}
    <button on:click={() => requestList(viewing)}>Refresh</button>
    <button on:click={() => (expanded = Object.create(null))}>Collapse All</button>
  {/if}
  <span class="count">
    {#if summary}
      {plural(summary.folders, 'folder')} · {plural(summary.files, 'file')} ·
      {formatSize(summary.size)}
    {/if}
    {#if request && !request.failed}
      {treeLoaded ? 'Refreshing…' : `Waiting for ${viewing}'s share list…`}
    {/if}
  </span>
  {#if request?.failed}<span class="notice">{request.failed}</span>{/if}
</div>

{#if treeLoaded}
  <div class="split">
    <div class="side wide">
      <div class="toolbar">
        <input {...noAutofill}
          class="fill"
          placeholder="Filter folders…"
          bind:value={filter}
          on:keydown={onEnter(applyFilter)}
          on:keydown={(e) => e.key === 'Escape' && clearFilter()}
        />
        {#if flatFolders}<button on:click={clearFilter}>Show Tree</button>{/if}
      </div>
      {#if flatFolders}
        <span class="count">
          {#if flatFolders.folders.length < flatFolders.total}
            First {flatFolders.folders.length.toLocaleString()} of
          {/if}
          {plural(flatFolders.total, 'matching folder')}
        </span>
        <DataTable
          id="browse-folders"
          label="Matching folders"
          rows={flatFolders.folders}
          key={(f) => f.directory}
          columns={folderColumns}
          viewKey={viewing}
          bind:selection={flatSelection}
          copyText={(f) => f.directory}
          empty="No folder matches “{filter.trim()}”."
          onpick={(folder) => selectFolder(folder.directory, folder.file_count > 0)}
          onactivate={revealFromFilter}
          onmenu={(rows, point) => openMenuAt(point, [
            { label: 'Show in Tree', primary: true, action: () => revealFromFilter(rows) },
            { sep: true },
            ...folderItems(rows[0].directory),
          ])}
        />
      {:else}
        <Tree
          label="Folders shared by {viewing}"
          nodes={visible}
          selected={selectedDir}
          {expanded}
          primary
          onselect={(node) => selectFolder(node.path, node.file_count > 0)}
          ontoggle={toggle}
          onmenu={(node, point) => openMenuAt(point, folderItems(node.path))}
        />
      {/if}
    </div>
    <div class="main">
      {#if selectedDir}
        <span class="path" title={selectedDir}>{selectedDir}</span>
        <DataTable
          id="browse-files"
          label="Files in {selectedDir}"
          rows={listing && listing.dir === selectedDir ? listing.files : []}
          key={(f) => f.name}
          {columns}
          viewKey={`${viewing}\u0000${selectedDir}`}
          bind:selection={fileSelection}
          typeahead={(f) => f.name}
          copyText={(f) => `${listing.dir}\\${f.name}`}
          loading={!listing || listing.dir !== selectedDir}
          empty="No files in this folder."
          onactivate={downloadFiles}
          onmenu={fileMenu}
        />
      {/if}
    </div>
  </div>
{/if}
