<script>
  import { onMount } from 'svelte';
  import { settings, status, applyTheme } from '../lib/stores.js';
  import { put, post, get } from '../lib/api.js';

  const pages = [
    ['network', 'Network'],
    ['shares', 'Shares'],
    ['downloads', 'Downloads'],
    ['uploads', 'Uploads'],
    ['searches', 'Searches'],
    ['filtering', 'Filtering'],
    ['notifications', 'Notifications'],
    ['profile', 'User Profile'],
    ['ui', 'User Interface'],
  ];
  const filterLevels = [
    ['open', 'Open', 'Never restrict anyone.'],
    ['guarded', 'Guarded', 'Block search flooding, and repeated same file downloads.'],
    ['strict', 'Strict', "Block peers sharing nothing or faking their share counts in addition to guarded."],
  ];
  const themes = [
    ['dark', 'Dark'],
    ['light', 'Light'],
    ['catppuccin', 'Catppuccin'],
  ];

  let page = 'network';
  let draft = null;
  let shareFiltersText = '';
  let newShare = { virtual_name: '', path: '', buddy_only: false };
  let saveError = '';
  let saved = false;
  let ipBans = [];
  let newIpBan = '';
  let ipBanError = '';
  let testStatus = null;

  onMount(async () => {
    ipBans = (await get('/ip_bans')).patterns;
  });

  async function addIpBan() {
    if (!newIpBan.trim()) return;
    ipBanError = '';
    try {
      await post('/ip_bans', { pattern: newIpBan.trim() });
      newIpBan = '';
    } catch (error) {
      if (error.status !== 400) throw error;
      ipBanError = 'Invalid pattern: use four dot-separated octets, * as wildcard.';
      return;
    }
    ipBans = (await get('/ip_bans')).patterns;
  }

  async function removeIpBan(pattern) {
    await post('/ip_bans/remove', { pattern });
    ipBans = (await get('/ip_bans')).patterns;
  }

  $: if (!draft && $settings.settings) {
    draft = { ...structuredClone($settings.settings), password: '' };
    shareFiltersText = draft.share_filters.join('\n');
  }
  $: locked = new Set($settings.locked);
  $: portManaged = $settings.gluetun || locked.has('listen_port');
  $: if (draft && portManaged && $settings.settings) {
    draft.listen_port = $settings.settings.listen_port;
  }

  function addShare() {
    if (!newShare.virtual_name.trim() || !newShare.path.trim()) return;
    draft.shares = [...draft.shares, { ...newShare }];
    newShare = { virtual_name: '', path: '', buddy_only: false };
  }

  function removeShare(index) {
    draft.shares = draft.shares.filter((_, i) => i !== index);
  }

  function selectTheme() {
    applyTheme(draft.theme);
  }

  async function sendTestNotification() {
    testStatus = null;
    try {
      await post('/pushover/test');
      testStatus = { message: 'Test notification sent.', failed: false };
    } catch (error) {
      testStatus = { message: error.message, failed: true };
    }
  }

  function revert() {
    draft = { ...structuredClone($settings.settings), password: '' };
    shareFiltersText = draft.share_filters.join('\n');
    applyTheme(draft.theme);
    saveError = '';
    saved = false;
  }

  async function save() {
    saveError = '';
    saved = false;
    const payload = {
      ...draft,
      incomplete_dir: draft.incomplete_dir || null,
      share_filters: shareFiltersText.split('\n').map((line) => line.trim()).filter(Boolean),
    };
    if (!payload.password) delete payload.password;
    try {
      await put('/settings', payload);
      saved = true;
    } catch (error) {
      saveError = error.message;
    }
  }
</script>

{#if draft}
  <div class="split">
    <div class="side">
      <div class="list">
        {#each pages as [id, label]}
          <div class:selected={page === id} on:click={() => (page = id)}>{label}</div>
        {/each}
      </div>
    </div>

    <div class="main">
      <div class="scroll settings-page">
        {#if page === 'network'}
          <h3>Network</h3>
          <div class="form-row">
            <label for="set-username">Username</label>
            <input id="set-username" bind:value={draft.username} disabled={locked.has('username')} />
            {#if locked.has('username')}<span class="hint">set by environment</span>{/if}
          </div>
          <div class="form-row">
            <label for="set-password">Password</label>
            <input id="set-password" type="password" bind:value={draft.password}
              placeholder={draft.password_set ? 'unchanged' : ''}
              disabled={locked.has('password')} />
            {#if locked.has('password')}<span class="hint">set by environment</span>{/if}
          </div>
          <div class="form-row">
            <label for="set-server">Server address</label>
            <input id="set-server" bind:value={draft.server} disabled={locked.has('server')} />
            {#if locked.has('server')}<span class="hint">set by environment</span>{/if}
          </div>
          <div class="form-row">
            <label for="set-port">Listen port</label>
            <input id="set-port" type="number" min="1024" max="65535"
              bind:value={draft.listen_port} disabled={portManaged} />
            {#if $settings.gluetun}<span class="hint">managed by Gluetun port forwarding</span>
            {:else if locked.has('listen_port')}<span class="hint">set by environment</span>{/if}
          </div>
          <div class="form-row">
            <label for="set-reconnect">
              <input id="set-reconnect" type="checkbox" bind:checked={draft.auto_reconnect} />
              Reconnect automatically when the connection is lost
            </label>
          </div>
          <p class="hint">
            Changing the username, password or server reconnects to the Soulseek network.
          </p>

        {:else if page === 'shares'}
          <h3>Shares</h3>
          <table>
            <thead>
              <tr><th>Virtual Folder</th><th class="grow">Folder</th><th>Buddies Only</th><th></th></tr>
            </thead>
            <tbody>
              {#each draft.shares as share, i}
                <tr>
                  <td>{share.virtual_name}</td>
                  <td class="grow">{share.path}</td>
                  <td><input type="checkbox" bind:checked={share.buddy_only} /></td>
                  <td><button on:click={() => removeShare(i)}>Remove</button></td>
                </tr>
              {/each}
              <tr>
                <td><input placeholder="Virtual name…" bind:value={newShare.virtual_name} /></td>
                <td class="grow"><input placeholder="Folder path on the server…" style="width: 100%"
                  bind:value={newShare.path} /></td>
                <td><input type="checkbox" bind:checked={newShare.buddy_only} /></td>
                <td><button on:click={addShare}>Add</button></td>
              </tr>
            </tbody>
          </table>
          <div class="toolbar">
            <button disabled={$status.scanning} on:click={() => post('/shares/rescan')}>
              {$status.scanning ? 'Scanning…' : 'Rescan Shares Now'}
            </button>
            <span class="hint">Saving share or exclusion changes starts a full rescan.</span>
          </div>
          <div class="form-row">
            <label for="set-share-filters">Excluded names</label>
            <textarea id="set-share-filters" rows="4" bind:value={shareFiltersText}
              placeholder="Thumbs.db&#10;desktop.ini"></textarea>
            <span class="hint">One exact file or folder name per line, skipped when scanning.</span>
          </div>
          <div class="form-row">
            <label for="set-scan-startup">
              <input id="set-scan-startup" type="checkbox" bind:checked={draft.scan_on_startup} />
              Scan shares on startup
            </label>
            <span class="hint">
              Until a scan runs you share nothing and appear to have no files.
            </span>
          </div>
          <div class="form-row">
            <label for="set-rescan-daily">
              <input id="set-rescan-daily" type="checkbox" bind:checked={draft.rescan_daily} />
              Rescan shares automatically every day
            </label>
          </div>
          <div class="form-row">
            <label for="set-rescan-hour">Daily rescan hour (UTC)</label>
            <input id="set-rescan-hour" type="number" min="0" max="23"
              bind:value={draft.rescan_hour_utc} />
          </div>

        {:else if page === 'downloads'}
          <h3>Downloads</h3>
          <div class="form-row">
            <label for="set-downdir">Download folder</label>
            <input id="set-downdir" bind:value={draft.download_dir}
              disabled={locked.has('download_dir')} />
            {#if locked.has('download_dir')}<span class="hint">set by environment</span>{/if}
          </div>
          <div class="form-row">
            <label for="set-incompletedir">Incomplete file folder</label>
            <input id="set-incompletedir" bind:value={draft.incomplete_dir}
              placeholder="{draft.download_dir}/incomplete" />
          </div>
          <div class="form-row">
            <label for="set-downlimit">Download speed limit (KiB/s)</label>
            <input id="set-downlimit" type="number" min="0" bind:value={draft.download_limit_kbps} />
            <span class="hint">0 = unlimited</span>
          </div>
          <div class="form-row">
            <label for="set-userdirs">
              <input id="set-userdirs" type="checkbox"
                bind:checked={draft.download_username_subfolders} />
              Place finished downloads in subfolders named after the uploader
            </label>
          </div>
          <div class="form-row">
            <label for="set-autoclear-down">
              <input id="set-autoclear-down" type="checkbox"
                bind:checked={draft.autoclear_downloads} />
              Clear finished downloads from the list automatically
            </label>
          </div>

        {:else if page === 'uploads'}
          <h3>Uploads</h3>
          <div class="form-row">
            <label for="set-slots">Upload slots (concurrent users)</label>
            <input id="set-slots" type="number" min="1" bind:value={draft.upload_slots} />
            <span class="hint">each user transfers one file at a time</span>
          </div>
          <div class="form-row">
            <label for="set-queuelimit">Queue limit per user (files)</label>
            <input id="set-queuelimit" type="number" min="1" bind:value={draft.queue_file_limit} />
          </div>
          <div class="form-row">
            <label for="set-uplimit">Upload speed limit (KiB/s)</label>
            <input id="set-uplimit" type="number" min="0" bind:value={draft.upload_limit_kbps} />
            <span class="hint">0 = unlimited</span>
          </div>
          <div class="form-row">
            <label for="set-queuemb">Queue limit per user (MiB)</label>
            <input id="set-queuemb" type="number" min="0" bind:value={draft.queue_size_limit_mb} />
            <span class="hint">0 = unlimited</span>
          </div>
          <div class="form-row">
            <label for="set-autoclear-up">
              <input id="set-autoclear-up" type="checkbox"
                bind:checked={draft.autoclear_uploads} />
              Clear finished uploads from the list automatically
            </label>
          </div>

        {:else if page === 'searches'}
          <h3>Searches</h3>
          <div class="form-row">
            <label for="set-respond">
              <input id="set-respond" type="checkbox" bind:checked={draft.respond_to_searches} />
              Respond to search requests from other users
            </label>
          </div>
          <div class="form-row">
            <label for="set-maxresults">Maximum results sent per search</label>
            <input id="set-maxresults" type="number" min="1"
              bind:value={draft.max_search_results} />
          </div>
          <div class="form-row">
            <label for="set-minchars">Minimum search term length</label>
            <input id="set-minchars" type="number" min="1" bind:value={draft.min_search_chars} />
          </div>
          <div class="form-row">
            <label for="set-maxresponses">Maximum responses kept per own search</label>
            <input id="set-maxresponses" type="number" min="1"
              bind:value={draft.max_search_responses} />
          </div>

        {:else if page === 'filtering'}
          <h3>Client Filtering</h3>
          {#each filterLevels as [id, label, description]}
            <div class="form-row">
              <label>
                <input type="radio" name="filter-level" value={id} bind:group={draft.filter_level} />
                {label}
              </label>
              <span class="hint">{description}</span>
            </div>
          {/each}
          <h3>Denial Messages</h3>
          <p class="hint">
            Sent to a peer when they try to queue a download. A peer matching more than one
            gets the first that applies, in this order.
          </p>
          <div class="form-row">
            <label for="set-banned">Banned</label>
            <input id="set-banned" style="width: 100%" bind:value={draft.banned_message} />
          </div>
          <div class="form-row">
            <label for="set-abusive">Abusive</label>
            <input id="set-abusive" style="width: 100%" bind:value={draft.abusive_message} />
          </div>
          <div class="form-row">
            <label for="set-leech">Leech</label>
            <input id="set-leech" style="width: 100%" bind:value={draft.leech_message} />
          </div>
          <div class="form-row">
            <label for="set-clear-on-dm">
              <input id="set-clear-on-dm" type="checkbox"
                bind:checked={draft.clear_verdict_on_message} />
              Clear a peer's verdict when they send a private message
            </label>
          </div>

          <h3>IP Bans</h3>
          <table>
            <thead>
              <tr><th class="grow">Pattern</th><th></th></tr>
            </thead>
            <tbody>
              {#each ipBans as pattern (pattern)}
                <tr>
                  <td class="grow">{pattern}</td>
                  <td><button on:click={() => removeIpBan(pattern)}>Remove</button></td>
                </tr>
              {/each}
              <tr>
                <td class="grow">
                  <input placeholder="192.168.1.1 or 10.0.*.*" bind:value={newIpBan}
                    on:keydown={(e) => e.key === 'Enter' && addIpBan()} />
                </td>
                <td><button on:click={addIpBan}>Add</button></td>
              </tr>
            </tbody>
          </table>
          {#if ipBanError}<p class="notice">{ipBanError}</p>{/if}
          <p class="hint">Connections from banned addresses are dropped immediately.</p>

        {:else if page === 'notifications'}
          <h3>Pushover</h3>
          <p class="hint">
            Sends push notifications through pushover.net. Register an application there for the API token and user key.
          </p>
          <div class="form-row">
            <label for="set-pushover-token">API token</label>
            <input id="set-pushover-token" style="width: 100%"
              bind:value={draft.pushover_token} />
          </div>
          <div class="form-row">
            <label for="set-pushover-user">User key</label>
            <input id="set-pushover-user" style="width: 100%"
              bind:value={draft.pushover_user_key} />
          </div>
          <div class="form-row">
            <label for="set-pushover-dms">
              <input id="set-pushover-dms" type="checkbox"
                bind:checked={draft.pushover_private_messages} />
              Send a notification when a private message arrives
            </label>
          </div>
          <div class="form-row">
            <button on:click={sendTestNotification}>Send Test Notification</button>
            {#if testStatus}
              <span class={testStatus.failed ? 'notice' : 'hint'}>{testStatus.message}</span>
            {/if}
          </div>

        {:else if page === 'profile'}
          <h3>User Profile</h3>
          <p class="hint">Shown to users who view your profile.</p>
          <textarea rows="10" bind:value={draft.description}></textarea>
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
          <h3>User Interface</h3>
          <div class="form-row">
            <label for="set-theme">Theme</label>
            <select id="set-theme" bind:value={draft.theme} on:change={selectTheme}>
              {#each themes as [id, label]}
                <option value={id}>{label}</option>
              {/each}
            </select>
          </div>
        {/if}
      </div>

      <div class="toolbar">
        <button on:click={save}>Save</button>
        <button on:click={revert}>Revert</button>
        {#if saveError}<span class="notice">{saveError}</span>
        {:else if saved}<span class="hint">Saved.</span>{/if}
      </div>
    </div>
  </div>
{:else}
  <p>Loading settings…</p>
{/if}
