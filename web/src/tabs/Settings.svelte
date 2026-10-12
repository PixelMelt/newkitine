<script>
  import { onDestroy, tick } from 'svelte';
  import { settings, status, previewTheme } from '../lib/stores.js';
  import { route, go, noAutofill } from '../lib/ui.js';
  import { put, post } from '../lib/api.js';
  import { act } from '../lib/act.js';
  import { onEnter, hasCommand } from '../lib/keys.js';
  import NumberField from '../lib/NumberField.svelte';

  const pages = [
    ['network', 'Network'],
    ['shares', 'Shares'],
    ['downloads', 'Downloads'],
    ['uploads', 'Uploads'],
    ['searches', 'Searches'],
    ['filtering', 'Filtering'],
    ['notifications', 'Notifications'],
    ['profile', 'My Profile'],
    ['ui', 'Appearance'],
  ];
  const NO_LIMIT = '0 means no limit.';
  const fields = {
    network: [
      { h: 'Network' },
      { text: 'username', label: 'Username' },
      { custom: 'password', keys: ['password'] },
      { text: 'server', label: 'Server address' },
      { number: 'listen_port', label: 'Listen port', min: 1024, max: 65535 },
      { check: 'auto_reconnect', label: 'Reconnect automatically when the connection is lost' },
      { note: 'Saving a changed username, password or server reconnects to the Soulseek network.' },
    ],
    shares: [
      { h: 'Shared Folders' },
      { custom: 'shares', keys: ['shares', 'share_filters'] },
      {
        check: 'scan_on_startup', label: 'Rescan shares on startup',
        hint: 'The last scan is loaded from cache at startup either way. Turn this on to also walk the shared folders on every start.',
      },
      { check: 'rescan_daily', label: 'Rescan shares every day' },
      { number: 'rescan_hour_utc', label: 'Daily rescan hour', min: 0, max: 23, unit: ':00 UTC' },
    ],
    downloads: [
      { h: 'Downloads' },
      { text: 'download_dir', label: 'Download folder', long: true },
      {
        text: 'incomplete_dir', label: 'Unfinished files folder', long: true,
        placeholder: (d) => `${d.download_dir}/incomplete`,
      },
      { number: 'download_limit_kbps', label: 'Download speed limit', min: 0, unit: 'KiB/s', hint: NO_LIMIT },
      { check: 'download_username_subfolders', label: 'Put finished downloads in a subfolder named after the uploader' },
      { check: 'autoclear_downloads', label: 'Clear finished downloads from the list automatically' },
    ],
    uploads: [
      { h: 'Uploads' },
      {
        number: 'upload_slots', label: 'Upload slots', min: 1, unit: 'users at once',
        hint: 'Each user gets one file at a time.',
      },
      { number: 'upload_limit_kbps', label: 'Upload speed limit', min: 0, unit: 'KiB/s', hint: NO_LIMIT },
      { h: 'Queue Limits per User' },
      { number: 'queue_file_limit', label: 'Files', min: 1, unit: 'files' },
      { number: 'queue_size_limit_mb', label: 'Total size', min: 0, unit: 'MiB', hint: NO_LIMIT },
      { check: 'autoclear_uploads', label: 'Clear finished uploads from the list automatically' },
    ],
    searches: [
      { h: "Answering Other Users' Searches" },
      { check: 'respond_to_searches', label: 'Answer searches from other users' },
      { number: 'max_search_results', label: 'Send at most', min: 1, max: 300, unit: 'results per search' },
      { number: 'min_search_chars', label: 'Ignore searches shorter than', min: 1, unit: 'characters' },
      { h: 'Your Searches' },
      { number: 'max_search_responses', label: 'Keep at most', min: 1, max: 2000, unit: 'responses per search' },
    ],
    filtering: [
      { h: 'Client Filtering' },
      { custom: 'levels', keys: ['filter_level'] },
      { note: 'At every level a peer who has already received the same file three times within two weeks is refused that file, and only that file, until the window passes or they message you.' },
      { h: 'Denial Messages' },
      { note: 'Sent to a peer when they try to queue a download. A peer matching more than one gets the first that applies, in this order.' },
      { text: 'banned_message', label: 'Banned', long: true },
      { text: 'abusive_message', label: 'Abusive', long: true },
      { text: 'leech_message', label: 'Leech', long: true },
      { check: 'clear_verdict_on_message', label: "Clear a peer's verdict when they send a private message" },
    ],
    notifications: [
      { h: 'Pushover' },
      { note: 'Sends push notifications through pushover.net. Register an application there to get the API token and user key.' },
      { text: 'pushover_token', label: 'API token', long: true },
      { text: 'pushover_user_key', label: 'User key', long: true },
      { check: 'pushover_private_messages', label: 'Notify me when a private message arrives' },
      { custom: 'pushover_test' },
    ],
    profile: [{ h: 'My Profile' }, { custom: 'profile', keys: ['description'] }],
    ui: [{ h: 'Appearance' }, { custom: 'theme', keys: ['theme'] }],
  };
  const keysOf = (id) => fields[id].flatMap((f) => f.keys ?? f.text ?? f.number ?? f.check ?? []);
  const filterLevels = [
    ['open', 'Open', 'Never restricts anyone.'],
    ['guarded', 'Guarded', 'Blocks search flooding.'],
    ['strict', 'Strict', 'Also blocks peers who share nothing or fake their share counts.'],
  ];
  const themes = [
    ['dark', 'Dark'],
    ['light', 'Light'],
    ['catppuccin', 'Catppuccin'],
  ];
  let draft = null;
  let version = 0;
  let baseline = null;
  let shareFiltersText = '';
  let removed = new Set();
  let newShare = { virtual_name: '', path: '', buddy_only: false };
  let invalid = Object.create(null);
  let saveError = '';
  let saving = false;
  let savedAt = null;
  let discarded = null;
  let testStatus = null;

  function payloadOf(source, filtersText, dropped) {
    const payload = {
      ...source,
      shares: source.shares.filter((share) => !dropped.has(share)),
      incomplete_dir: source.incomplete_dir || null,
      share_filters: filtersText.split('\n').map((line) => line.trim()).filter(Boolean),
    };
    if (!payload.password) delete payload.password;
    return payload;
  }

  function adopt(server) {
    draft = { ...structuredClone(server), password: '' };
    shareFiltersText = draft.share_filters.join('\n');
    removed = new Set();
    baseline = structuredClone(payloadOf(draft, shareFiltersText, removed));
    version += 1;
  }

  function rebase(server) {
    for (const key of Object.keys(server)) {
      if (key === 'shares' || key === 'share_filters') continue;
      if (JSON.stringify(draft[key]) !== JSON.stringify(baseline[key])) continue;
      draft[key] = structuredClone(server[key]);
      baseline[key] = structuredClone(server[key]);
    }
  }

  const isDirty = () =>
    JSON.stringify(payloadOf(draft, shareFiltersText, removed)) !== JSON.stringify(baseline);

  onDestroy(
    settings.subscribe((value) => {
      if (!value.settings) return;
      const typing = newShare.virtual_name.trim() || newShare.path.trim();
      if (!draft || (!saving && !typing && !isDirty())) adopt(value.settings);
      else if (!saving) rebase(value.settings);
      if (value.gluetun || value.locked.includes('listen_port')) {
        draft.listen_port = value.settings.listen_port;
        baseline.listen_port = value.settings.listen_port;
      }
    }),
  );

  $: page = pages.some(([id]) => id === $route.items.settings) ? $route.items.settings : 'network';
  $: locked = new Set($settings.locked);
  $: lockNote = (key) =>
    key === 'listen_port' && $settings.gluetun
      ? 'Managed by Gluetun port forwarding.'
      : locked.has(key) ? `Set by NEWKITINE_${key.toUpperCase()}.` : '';
  $: current = draft ? payloadOf(draft, shareFiltersText, removed) : null;
  $: pendingShare = Boolean(newShare.virtual_name.trim() || newShare.path.trim());
  $: dirty = draft ? pendingShare || JSON.stringify(current) !== JSON.stringify(baseline) : false;
  $: if (dirty) {
    savedAt = null;
    discarded = null;
  }
  $: pushoverChanged =
    draft && keysOf('notifications').some((key) => current[key] !== baseline[key]);
  function addShare() {
    if (!newShare.virtual_name.trim() || !newShare.path.trim()) return false;
    draft.shares = [...draft.shares, { ...newShare }];
    newShare = { virtual_name: '', path: '', buddy_only: false };
    return true;
  }

  function toggleRemoved(share) {
    if (!removed.delete(share)) removed.add(share);
    removed = removed;
  }

  function discard() {
    if (!dirty) return;
    discarded = { draft, shareFiltersText, removed, newShare };
    newShare = { virtual_name: '', path: '', buddy_only: false };
    adopt($settings.settings);
    previewTheme(draft.theme);
    saveError = '';
  }

  function restoreDiscarded() {
    ({ draft, shareFiltersText, removed, newShare } = discarded);
    version += 1;
    previewTheme(draft.theme);
    discarded = null;
  }

  async function save() {
    saveError = '';
    document.activeElement.blur();
    await tick();
    const halfShare = Boolean(newShare.virtual_name.trim()) !== Boolean(newShare.path.trim());
    if (halfShare) {
      go('settings', 'shares');
      saveError = 'Not saved: finish or clear the new share row first.';
      return false;
    }
    addShare();
    const badField = Object.keys(invalid).find((key) => invalid[key]);
    if (badField) {
      go('settings', pages.find(([id]) => keysOf(id).includes(badField))[0]);
      saveError = 'Not saved: fix the outlined number first.';
      return false;
    }
    await tick();
    if (!dirty) return true;
    const payload = structuredClone(payloadOf(draft, shareFiltersText, removed));
    const dropped = new Set(removed);
    saving = true;
    const saved = await act(() => put('/settings', payload), {
      failed: 'Preferences not saved',
      where: (text, error) => {
        saveError = text;
        if (error.status !== 400 && error.status !== 422) return;
        const owner = pages.find(([id]) =>
          keysOf(id).some((key) => new RegExp(`\\b${key}\\b`).test(error.message)),
        );
        if (owner) go('settings', owner[0]);
        else if (/\bshare (name|filter)\b/.test(error.message)) go('settings', 'shares');
      },
    });
    saving = false;
    if (!saved) return false;
    const sent = structuredClone(payload);
    draft.password_set = draft.password_set || Boolean(payload.password);
    if (draft.password === (payload.password ?? '')) draft.password = '';
    draft.shares = draft.shares.filter((share) => !dropped.has(share));
    removed = new Set([...removed].filter((share) => !dropped.has(share)));
    delete sent.password;
    sent.password_set = draft.password_set;
    baseline = sent;
    await tick();
    savedAt = Date.now();
    return true;
  }

  async function sendTestNotification() {
    testStatus = null;
    if (pushoverChanged && !(await save())) return;
    await act(() => post('/pushover/test'), {
      failed: 'Test notification not sent',
      where: (text) => (testStatus = { message: text, failed: true }),
      then: () => (testStatus = { message: 'Test notification sent. Check your device.', failed: false }),
    });
  }

  function windowKey(event) {
    if ($route.tab !== 'settings' || !hasCommand(event) || event.key.toLowerCase() !== 's') return;
    event.preventDefault();
    save();
  }

  function pageKey(event) {
    const index = pages.findIndex(([id]) => id === page);
    const next = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: pages.length - 1 }[event.key];
    if (next === undefined || next < 0 || next >= pages.length) return;
    event.preventDefault();
    go('settings', pages[next][0]);
  }

  function beforeUnload(event) {
    if (dirty) event.preventDefault();
  }
</script>

<svelte:window on:keydown={windowKey} on:beforeunload={beforeUnload} />

{#if draft}
  <div class="split">
    <div class="side nav">
      <div
        class="list pick"
        role="listbox"
        aria-label="Preference pages"
        tabindex="0"
        data-primary
        on:keydown={pageKey}
      >
        {#each pages as [id, label]}
          <div
            role="option"
            aria-selected={page === id}
            class:selected={page === id}
            on:mousedown={() => go('settings', id)}
          >
            {label}
          </div>
        {/each}
      </div>
    </div>

    <div class="main">
      <div class="scroll settings-page">
        {#key version}
        {#each fields[page] as f (f)}
          {#if f.h}
            <h3>{f.h}</h3>
          {:else if f.note}
            <p class="hint">{f.note}</p>
          {:else if f.check}
            <div class="form-row check">
              <label>
                <input type="checkbox" bind:checked={draft[f.check]} />
                {f.label}
              </label>
              {#if f.hint}<span class="hint">{f.hint}</span>{/if}
            </div>
          {:else if f.text || f.number}
            {@const key = f.text ?? f.number}
            <div class="form-row">
              <label for="set-{key}">{f.label}</label>
              <div class="control">
                {#if f.number}
                  <NumberField id="set-{key}" min={f.min} max={f.max ?? null} unit={f.unit ?? ''}
                    locked={Boolean(lockNote(key))} bind:value={draft[key]} bind:invalid={invalid[key]} />
                {:else}
                  <input {...noAutofill} id="set-{key}" class:long={f.long} type="text" bind:value={draft[key]}
                    placeholder={f.placeholder?.(draft)} readonly={Boolean(lockNote(key))} />
                {/if}
                {#if lockNote(key)}<span class="hint">{lockNote(key)}</span>{/if}
                {#if f.hint}<span class="hint">{f.hint}</span>{/if}
              </div>
            </div>
          {:else if f.custom === 'password'}
            <div class="form-row">
              <label for="set-password">Password</label>
              <div class="control">
                <input {...noAutofill} autocomplete="new-password" id="set-password" type="password"
                  bind:value={draft.password} placeholder={draft.password_set ? 'unchanged' : ''}
                  readonly={Boolean(lockNote('password'))} />
                {#if lockNote('password')}<span class="hint">{lockNote('password')}</span>{/if}
              </div>
            </div>
          {:else if f.custom === 'shares'}
            <table class="editor">
              <thead>
                <tr><th>Name shown to others</th><th class="grow">Folder on the server</th><th>Buddies only</th><th></th></tr>
              </thead>
              <tbody>
                {#each draft.shares as share}
                  <tr class:removed={removed.has(share)}>
                    <td><input type="text" {...noAutofill} bind:value={share.virtual_name} /></td>
                    <td class="grow"><input type="text" {...noAutofill} bind:value={share.path} /></td>
                    <td><input type="checkbox" aria-label="Buddies only" bind:checked={share.buddy_only} /></td>
                    <td>
                      <button type="button" on:click={() => toggleRemoved(share)}>
                        {removed.has(share) ? 'Restore' : 'Remove'}
                      </button>
                    </td>
                  </tr>
                {/each}
              </tbody>
              <tfoot>
                <tr>
                  <td><input type="text" {...noAutofill} placeholder="Music"
                    bind:value={newShare.virtual_name} on:keydown={onEnter(addShare)} /></td>
                  <td class="grow"><input type="text" {...noAutofill} placeholder="/data/shared/music" bind:value={newShare.path} on:keydown={onEnter(addShare)} /></td>
                  <td><input type="checkbox" aria-label="New share is buddies only" bind:checked={newShare.buddy_only} /></td>
                  <td><button type="button" on:click={addShare}>Add</button></td>
                </tr>
              </tfoot>
            </table>
            <div class="form-row">
              <span></span>
              <div class="control">
                <button type="button" disabled={$status.scanning}
                  on:click={() => act(() => post('/shares/rescan'), { failed: 'Rescan not started' })}>
                  {$status.scanning ? 'Scanning…' : 'Rescan Shares Now'}
                </button>
                <span class="hint">Saving share or exclusion changes starts a full rescan.</span>
              </div>
            </div>
            <div class="form-row">
              <label for="set-share-filters">Excluded names</label>
              <div class="control">
                <textarea {...noAutofill} id="set-share-filters" rows="4" bind:value={shareFiltersText}
                  placeholder="Thumbs.db&#10;desktop.ini"></textarea>
                <span class="hint">One exact file or folder name per line.</span>
              </div>
            </div>
          {:else if f.custom === 'levels'}
            {#each filterLevels as [id, label, description]}
              <div class="form-row check">
                <label>
                  <input type="radio" name="filter-level" value={id} bind:group={draft.filter_level} />
                  {label}
                  <span class="hint">{description}</span>
                </label>
              </div>
            {/each}
          {:else if f.custom === 'pushover_test'}
            <div class="form-row">
              <span></span>
              <div class="control">
                <button type="button" on:click={sendTestNotification}>
                  {pushoverChanged ? 'Save and Send Test Notification' : 'Send Test Notification'}
                </button>
                {#if testStatus}
                  <span class={testStatus.failed ? 'notice' : 'hint'}>{testStatus.message}</span>
                {/if}
              </div>
            </div>
          {:else if f.custom === 'profile'}
            <textarea {...noAutofill} rows="10"
              bind:value={draft.description}></textarea>
            <p class="hint">
              $&#123;user.name&#125; inserts a value, $&#123;user.is_buddy?Hey buddy:Hi&#125; picks
              text from a yes/no flag, $$ writes a literal dollar sign. Values and flags are
              resolved for whoever asked, at the moment they ask.
            </p>
            <p class="hint">
              Values: user.name, user.restriction, user.queued_files, user.active_uploads, me.name,
              me.shared_files, me.shared_folders, me.queue_size, me.slots, me.free_slots,
              me.upload_speed.
            </p>
            <p class="hint">
              Flags: user.is_buddy, user.is_ignored, user.is_banned, user.is_privileged.
            </p>
          {:else}
            <div class="form-row">
              <label for="set-theme">Theme</label>
              <div class="control">
                <select id="set-theme" bind:value={draft.theme} on:change={() => previewTheme(draft.theme)}>
                  {#each themes as [id, label]}
                    <option value={id}>{label}</option>
                  {/each}
                </select>
              </div>
            </div>
          {/if}
        {/each}
        {/key}
      </div>

      <div class="toolbar savebar">
        <button type="button" on:click={save}>Save</button>
        {#if saveError}<span class="notice">{saveError}</span>
        {:else if dirty}<span>Unsaved changes.</span>
        {:else if discarded}
          <span>Changes discarded.</span>
          <button type="button" on:click={restoreDiscarded}>Undo</button>
        {:else if savedAt}<span class="hint">Saved.</span>{/if}
        <button type="button" class="spaced" on:click={discard}>Discard Changes</button>
      </div>
    </div>
  </div>
{:else}
  <p class="hint">Waiting for the Newkitine server to send the preferences…</p>
{/if}
