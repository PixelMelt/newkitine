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
  const pageFields = {
    network: ['username', 'password', 'server', 'listen_port', 'auto_reconnect'],
    shares: ['shares', 'share_filters', 'scan_on_startup', 'rescan_daily', 'rescan_hour_utc'],
    downloads: [
      'download_dir', 'incomplete_dir', 'download_limit_kbps', 'download_username_subfolders',
      'autoclear_downloads',
    ],
    uploads: [
      'upload_slots', 'queue_file_limit', 'upload_limit_kbps', 'queue_size_limit_mb',
      'autoclear_uploads',
    ],
    searches: ['respond_to_searches', 'max_search_results', 'min_search_chars', 'max_search_responses'],
    filtering: [
      'filter_level', 'banned_message', 'abusive_message', 'leech_message',
      'clear_verdict_on_message',
    ],
    notifications: ['pushover_token', 'pushover_user_key', 'pushover_private_messages'],
    profile: ['description'],
    ui: ['theme'],
  };
  const lockedBy = {
    server: 'NEWKITINE_SERVER',
    username: 'NEWKITINE_USERNAME',
    password: 'NEWKITINE_PASSWORD',
    listen_port: 'NEWKITINE_LISTEN_PORT',
    download_dir: 'NEWKITINE_DOWNLOAD_DIR',
  };
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

  const isDirty = () =>
    JSON.stringify(payloadOf(draft, shareFiltersText, removed)) !== JSON.stringify(baseline);

  onDestroy(
    settings.subscribe((value) => {
      if (!value.settings) return;
      if (!draft || !isDirty()) adopt(value.settings);
      else if (value.gluetun || value.locked.includes('listen_port')) {
        draft.listen_port = value.settings.listen_port;
        baseline.listen_port = value.settings.listen_port;
      }
    }),
  );

  $: page = pages.some(([id]) => id === $route.items.settings) ? $route.items.settings : 'network';
  $: locked = new Set($settings.locked);
  $: portManaged = $settings.gluetun || locked.has('listen_port');
  $: current = draft ? payloadOf(draft, shareFiltersText, removed) : null;
  $: pendingShare = Boolean(newShare.virtual_name.trim() || newShare.path.trim());
  $: dirty = draft ? pendingShare || JSON.stringify(current) !== JSON.stringify(baseline) : false;
  $: changedPages = new Set(
    draft
      ? pages
          .filter(([id]) =>
            pageFields[id].some((key) => JSON.stringify(current[key]) !== JSON.stringify(baseline[key])),
          )
          .map(([id]) => id)
      : [],
  );
  $: if (dirty) {
    savedAt = null;
    discarded = null;
  }
  $: pushoverChanged = changedPages.has('notifications');
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
      go('settings', pages.find(([id]) => pageFields[id].includes(badField))[0]);
      saveError = 'Not saved: fix the outlined number first.';
      return false;
    }
    await tick();
    if (!dirty) return true;
    const payload = structuredClone(payloadOf(draft, shareFiltersText, removed));
    const dropped = new Set(removed);
    const saved = await act(() => put('/settings', payload), {
      failed: 'Preferences not saved',
      where: (text, error) => {
        saveError = text;
        if (error.status !== 400 && error.status !== 422) return;
        const owner = pages.find(([id]) =>
          pageFields[id].some((key) => new RegExp(`\\b${key}\\b`).test(error.message)),
        );
        if (owner) go('settings', owner[0]);
        else if (/\bshare (name|filter)\b/.test(error.message)) go('settings', 'shares');
      },
    });
    if (!saved) return false;
    const sent = structuredClone(payload);
    draft.password_set = draft.password_set || Boolean(payload.password);
    draft.password = '';
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
        {#if page === 'network'}
          <h3>Network</h3>
          <div class="form-row">
            <label for="set-username">Username</label>
            <div class="control">
              <input {...noAutofill} id="set-username" type="text" bind:value={draft.username}
                readonly={locked.has('username')} />
              {#if locked.has('username')}<span class="hint">Set by {lockedBy.username}.</span>{/if}
            </div>
          </div>
          <div class="form-row">
            <label for="set-password">Password</label>
            <div class="control">
              <input {...noAutofill} autocomplete="new-password" id="set-password" type="password"
                bind:value={draft.password} placeholder={draft.password_set ? 'unchanged' : ''}
                readonly={locked.has('password')} />
              {#if locked.has('password')}<span class="hint">Set by {lockedBy.password}.</span>{/if}
            </div>
          </div>
          <div class="form-row">
            <label for="set-server">Server address</label>
            <div class="control">
              <input {...noAutofill} id="set-server" type="text" bind:value={draft.server}
                readonly={locked.has('server')} />
              {#if locked.has('server')}<span class="hint">Set by {lockedBy.server}.</span>{/if}
            </div>
          </div>
          <div class="form-row">
            <label for="set-port">Listen port</label>
            <div class="control">
              <NumberField id="set-port" min={1024} max={65535} locked={portManaged}
                bind:value={draft.listen_port} bind:invalid={invalid.listen_port} />
              {#if $settings.gluetun}<span class="hint">Managed by Gluetun port forwarding.</span>
              {:else if locked.has('listen_port')}<span class="hint">Set by {lockedBy.listen_port}.</span>{/if}
            </div>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.auto_reconnect} />
              Reconnect automatically when the connection is lost
            </label>
          </div>
          <p class="hint">
            Saving a changed username, password or server reconnects to the Soulseek network.
          </p>

        {:else if page === 'shares'}
          <h3>Shared Folders</h3>
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
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.scan_on_startup} />
              Rescan shares on startup
            </label>
            <span class="hint">
              The last scan is loaded from cache at startup either way. Turn this on to also walk
              the shared folders on every start.
            </span>
          </div>
          <div class="form-row check">
            <label for="set-rescan-daily">
              <input id="set-rescan-daily" type="checkbox" bind:checked={draft.rescan_daily} />
              Rescan shares every day
            </label>
          </div>
          <div class="form-row">
            <label for="set-rescan-hour">Daily rescan hour</label>
            <div class="control">
              <NumberField id="set-rescan-hour" min={0} max={23} unit=":00 UTC"
                bind:value={draft.rescan_hour_utc} bind:invalid={invalid.rescan_hour_utc} />
            </div>
          </div>

        {:else if page === 'downloads'}
          <h3>Downloads</h3>
          <div class="form-row">
            <label for="set-downdir">Download folder</label>
            <div class="control">
              <input {...noAutofill} id="set-downdir" class="long" type="text" bind:value={draft.download_dir}
                readonly={locked.has('download_dir')} />
              {#if locked.has('download_dir')}<span class="hint">Set by {lockedBy.download_dir}.</span>{/if}
            </div>
          </div>
          <div class="form-row">
            <label for="set-incompletedir">Unfinished files folder</label>
            <div class="control">
              <input {...noAutofill} id="set-incompletedir" class="long" type="text" bind:value={draft.incomplete_dir}
                placeholder="{draft.download_dir}/incomplete" />
            </div>
          </div>
          <div class="form-row">
            <label for="set-downlimit">Download speed limit</label>
            <div class="control">
              <NumberField id="set-downlimit" min={0} unit="KiB/s"
                bind:value={draft.download_limit_kbps} bind:invalid={invalid.download_limit_kbps} />
              <span class="hint">0 means no limit.</span>
            </div>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.download_username_subfolders} />
              Put finished downloads in a subfolder named after the uploader
            </label>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.autoclear_downloads} />
              Clear finished downloads from the list automatically
            </label>
          </div>

        {:else if page === 'uploads'}
          <h3>Uploads</h3>
          <div class="form-row">
            <label for="set-slots">Upload slots</label>
            <div class="control">
              <NumberField id="set-slots" min={1} unit="users at once"
                bind:value={draft.upload_slots} bind:invalid={invalid.upload_slots} />
              <span class="hint">Each user gets one file at a time.</span>
            </div>
          </div>
          <div class="form-row">
            <label for="set-uplimit">Upload speed limit</label>
            <div class="control">
              <NumberField id="set-uplimit" min={0} unit="KiB/s"
                bind:value={draft.upload_limit_kbps} bind:invalid={invalid.upload_limit_kbps} />
              <span class="hint">0 means no limit.</span>
            </div>
          </div>
          <h3>Queue Limits per User</h3>
          <div class="form-row">
            <label for="set-queuelimit">Files</label>
            <div class="control">
              <NumberField id="set-queuelimit" min={1} unit="files"
                bind:value={draft.queue_file_limit} bind:invalid={invalid.queue_file_limit} />
            </div>
          </div>
          <div class="form-row">
            <label for="set-queuemb">Total size</label>
            <div class="control">
              <NumberField id="set-queuemb" min={0} unit="MiB"
                bind:value={draft.queue_size_limit_mb} bind:invalid={invalid.queue_size_limit_mb} />
              <span class="hint">0 means no limit.</span>
            </div>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.autoclear_uploads} />
              Clear finished uploads from the list automatically
            </label>
          </div>

        {:else if page === 'searches'}
          <h3>Answering Other Users' Searches</h3>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.respond_to_searches} />
              Answer searches from other users
            </label>
          </div>
          <div class="form-row">
            <label for="set-maxresults">Send at most</label>
            <div class="control">
              <NumberField id="set-maxresults" min={1} max={300} unit="results per search"
                bind:value={draft.max_search_results} bind:invalid={invalid.max_search_results} />
            </div>
          </div>
          <div class="form-row">
            <label for="set-minchars">Ignore searches shorter than</label>
            <div class="control">
              <NumberField id="set-minchars" min={1} unit="characters"
                bind:value={draft.min_search_chars} bind:invalid={invalid.min_search_chars} />
            </div>
          </div>
          <h3>Your Searches</h3>
          <div class="form-row">
            <label for="set-maxresponses">Keep at most</label>
            <div class="control">
              <NumberField id="set-maxresponses" min={1} max={2000} unit="responses per search"
                bind:value={draft.max_search_responses} bind:invalid={invalid.max_search_responses} />
            </div>
          </div>

        {:else if page === 'filtering'}
          <h3>Client Filtering</h3>
          {#each filterLevels as [id, label, description]}
            <div class="form-row check">
              <label>
                <input type="radio" name="filter-level" value={id} bind:group={draft.filter_level} />
                {label}
                <span class="hint">{description}</span>
              </label>
            </div>
          {/each}
          <p class="hint">
            At every level a peer who has already received the same file three times within two
            weeks is refused that file, and only that file, until the window passes or they
            message you.
          </p>
          <h3>Denial Messages</h3>
          <p class="hint">
            Sent to a peer when they try to queue a download. A peer matching more than one gets
            the first that applies, in this order.
          </p>
          <div class="form-row">
            <label for="set-banned">Banned</label>
            <div class="control"><input {...noAutofill} id="set-banned" class="long" type="text" bind:value={draft.banned_message} /></div>
          </div>
          <div class="form-row">
            <label for="set-abusive">Abusive</label>
            <div class="control"><input {...noAutofill} id="set-abusive" class="long" type="text" bind:value={draft.abusive_message} /></div>
          </div>
          <div class="form-row">
            <label for="set-leech">Leech</label>
            <div class="control"><input {...noAutofill} id="set-leech" class="long" type="text" bind:value={draft.leech_message} /></div>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.clear_verdict_on_message} />
              Clear a peer's verdict when they send a private message
            </label>
          </div>

        {:else if page === 'notifications'}
          <h3>Pushover</h3>
          <p class="hint">
            Sends push notifications through pushover.net. Register an application there to get
            the API token and user key.
          </p>
          <div class="form-row">
            <label for="set-pushover-token">API token</label>
            <div class="control"><input {...noAutofill} id="set-pushover-token" class="long" type="text" bind:value={draft.pushover_token} /></div>
          </div>
          <div class="form-row">
            <label for="set-pushover-user">User key</label>
            <div class="control"><input {...noAutofill} id="set-pushover-user" class="long" type="text" bind:value={draft.pushover_user_key} /></div>
          </div>
          <div class="form-row check">
            <label>
              <input type="checkbox" bind:checked={draft.pushover_private_messages} />
              Notify me when a private message arrives
            </label>
          </div>
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

        {:else if page === 'profile'}
          <h3>My Profile</h3>
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

        {:else if page === 'ui'}
          <h3>Appearance</h3>
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
