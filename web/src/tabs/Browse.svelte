<script>
  import { browses } from '../lib/stores.js';
  import { browseTarget } from '../lib/ui.js';
  import { get, post, formatSize, formatAttributes } from '../lib/api.js';
  import { openMenu } from '../lib/menu.js';
  import { userMenu } from '../lib/usermenu.js';
  import { sortRows } from '../lib/sort.js';
  import Th from '../lib/Th.svelte';

  let sort = { key: null, dir: 1 };
  let username = '';
  let viewing = '';
  let filter = '';
  let tree = {};
  let expanded = {};
  let treeLoaded = false;
  let flatFolders = null;
  let currentDir = null;
  let files = null;
  let loadedAt = 0;
  let pendingFolder = null;
  let summary = null;

  $: if ($browseTarget) {
    const { username: target, folder } = $browseTarget;
    browseTarget.set(null);
    username = target;
    openUser(target, folder);
  }

  $: if (viewing && $browses[viewing] && $browses[viewing] !== loadedAt) {
    loadedAt = $browses[viewing];
    loadTree();
  }

  $: visible = flatten(tree, expanded, '', 0);

  function flatten(tree, expanded, path, depth) {
    const out = [];
    for (const node of tree[path] ?? []) {
      out.push({ node, depth });
      if (expanded[node.path]) out.push(...flatten(tree, expanded, node.path, depth + 1));
    }
    return out;
  }

  async function loadChildren(dir) {
    const data = await get(
      `/users/${encodeURIComponent(viewing)}/tree?dir=${encodeURIComponent(dir)}`);
    tree[dir] = data.children;
    tree = tree;
    if (data.summary) summary = data.summary;
  }

  async function loadTree() {
    tree = {};
    expanded = {};
    treeLoaded = false;
    await loadChildren('');
    treeLoaded = true;
    if (pendingFolder) {
      const folder = pendingFolder;
      pendingFolder = null;
      await revealFolder(folder);
    }
  }

  async function revealFolder(folder) {
    const parts = folder.split('\\');
    let path = '';
    for (const part of parts.slice(0, -1)) {
      path = path ? `${path}\\${part}` : part;
      if (!tree[path]) await loadChildren(path);
      expanded[path] = true;
    }
    expanded = expanded;
    selectFolder(folder, true);
  }

  async function toggle(node) {
    expanded[node.path] = !expanded[node.path];
    expanded = expanded;
    if (expanded[node.path] && !tree[node.path]) await loadChildren(node.path);
  }

  async function selectFolder(dir, hasFiles) {
    currentDir = dir;
    if (!hasFiles) {
      files = [];
      return;
    }
    files = null;
    const data = await get(
      `/users/${encodeURIComponent(viewing)}/files?dir=${encodeURIComponent(dir)}`);
    files = data.files;
  }

  async function openUser(target, folder = null) {
    viewing = target;
    filter = '';
    flatFolders = null;
    currentDir = null;
    files = null;
    loadedAt = 0;
    pendingFolder = folder;
    tree = {};
    expanded = {};
    treeLoaded = false;
    summary = null;
    try {
      await loadTree();
    } catch (error) {
      if (error.status !== 404) throw error;
      post(`/users/${encodeURIComponent(target)}/browse`);
    }
  }

  function requestBrowse() {
    if (!username.trim()) return;
    openUser(username.trim());
  }

  async function applyFilter() {
    if (!filter.trim()) {
      flatFolders = null;
      return;
    }
    flatFolders = await get(
      `/users/${encodeURIComponent(viewing)}/folders?filter=${encodeURIComponent(filter)}`);
  }

  function downloadFile(file) {
    post('/downloads', {
      username: viewing,
      virtual_path: currentDir + '\\' + file.name,
      size: file.size,
      attributes: file.attributes,
    });
  }

  function downloadFolder(dir, recursive) {
    post('/downloads/folder', { username: viewing, dir, recursive });
  }

  function folderMenu(event, dir) {
    openMenu(event, [
      { label: 'Download Folder', action: () => downloadFolder(dir, false) },
      { label: 'Download Folder & Subfolders', action: () => downloadFolder(dir, true) },
      { sep: true },
      { label: 'Copy Folder Path', action: () => navigator.clipboard.writeText(dir) },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(viewing, { skip: ['userbrowse'] }) },
    ]);
  }

  function fileMenu(event, file) {
    openMenu(event, [
      { label: 'Download File', action: () => downloadFile(file) },
      { sep: true },
      {
        label: 'Copy File Path',
        action: () => navigator.clipboard.writeText(currentDir + '\\' + file.name),
      },
      { sep: true },
      { label: 'User Actions', submenu: userMenu(viewing, { skip: ['userbrowse'] }) },
    ]);
  }
</script>

<div class="toolbar">
  <input
    placeholder="Username…"
    bind:value={username}
    on:keydown={(e) => e.key === 'Enter' && requestBrowse()}
  />
  <button on:click={requestBrowse}>Browse</button>
  {#if treeLoaded}
    <button on:click={() => post(`/users/${encodeURIComponent(viewing)}/browse`)}>
      Refresh
    </button>
    <button on:click={() => (expanded = {})}>Collapse All</button>
  {/if}
  {#if summary}
    <span>
      {summary.folders.toLocaleString()} folders ·
      {summary.files.toLocaleString()} files ·
      {formatSize(summary.size)}
    </span>
  {/if}
  {#if viewing && !treeLoaded}
    <span>Waiting for {viewing}'s share list…</span>
  {/if}
</div>

{#if treeLoaded}
  <div class="split">
    <div class="side" style="width: 40%;">
      <input
        placeholder="Filter folders…"
        bind:value={filter}
        on:keydown={(e) => e.key === 'Enter' && applyFilter()}
      />
      {#if flatFolders}
        <span>showing {flatFolders.folders.length} of {flatFolders.total} matches</span>
        <div class="list" tabindex="0">
          {#each flatFolders.folders as folder}
            <div
              class:selected={currentDir === folder.directory}
              on:click={() => selectFolder(folder.directory, folder.file_count > 0)}
              on:contextmenu={(e) => folderMenu(e, folder.directory)}
            >
              {folder.directory} ({folder.file_count})
            </div>
          {/each}
        </div>
      {:else}
        <div class="tree" tabindex="0">
          {#each visible as { node, depth }}
            <div
              class="node"
              class:selected={currentDir === node.path}
              style:padding-left="{6 + depth * 14}px"
              on:click={() => selectFolder(node.path, node.file_count > 0)}
              on:dblclick={() => node.has_children && toggle(node)}
              on:contextmenu={(e) => folderMenu(e, node.path)}
            >
              <span
                class="twisty"
                on:click|stopPropagation={() => node.has_children && toggle(node)}
              >
                {node.has_children ? (expanded[node.path] ? '[-]' : '[+]') : ''}
              </span>
              <span>
                {node.private ? '[PRIVATE] ' : ''}{node.name}
                {#if node.file_count}<span class="count">({node.file_count})</span>{/if}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
    <div class="main">
      {#if currentDir}
        <span>{currentDir}</span>
        <div class="scroll" tabindex="0">
          <table>
            <thead>
              <tr>
                <Th bind:sort key="name" grow>Filename</Th>
                <Th bind:sort key="size">Size</Th>
                <Th bind:sort key="quality">Quality</Th>
              </tr>
            </thead>
            <tbody>
              {#each sortRows(files ?? [], sort, { quality: (f) => formatAttributes(f.attributes) }) as file}
                <tr
                  class="clickable"
                  on:contextmenu={(e) => fileMenu(e, file)}
                  on:dblclick={() => downloadFile(file)}
                >
                  <td class="grow">{file.name}</td>
                  <td>{formatSize(file.size)}</td>
                  <td>{formatAttributes(file.attributes)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  </div>
{/if}
