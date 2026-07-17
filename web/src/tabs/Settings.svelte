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
    ['profile', 'User Profile'],
    ['ui', 'User Interface'],
  ];
  const filterLevels = [
    ['open', 'Open', 'Observe and record peer behavior, never restrict anyone.'],
    ['guarded', 'Guarded', 'Deny peers with hard evidence of faked shares; deprioritize suspicious ones. Real users are never blocked.'],
    ['strict', 'Strict', "Hold a peers's first download until their shares are checked. Peers sharing nothing or faking their share counts are denied."],
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
            <span class="hint">Saved share changes are rescanned automatically.</span>
          </div>
          <div class="form-row">
            <label for="set-share-filters">Excluded names</label>
            <textarea id="set-share-filters" rows="4" bind:value={shareFiltersText}
              placeholder="Thumbs.db&#10;desktop.ini"></textarea>
            <span class="hint">One exact file or folder name per line, skipped when scanning.</span>
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
            <label for="set-slots">Upload slots</label>
            <input id="set-slots" type="number" min="1" bind:value={draft.upload_slots} />
          </div>
          <div class="form-row">
            <label for="set-peruser">Uploads per user</label>
            <input id="set-peruser" type="number" min="0" bind:value={draft.uploads_per_user} />
            <span class="hint">0 = unlimited</span>
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
            <label for="set-banned">Ban message</label>
            <input id="set-banned" style="width: 100%" bind:value={draft.banned_message} />
            <span class="hint">Sent to banned users when they try to queue a download.</span>
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
          <p class="hint">
            Restricts clients that take without participating: faked share stats, search
            scraping, zero shares. Buddies and users you have downloaded from are never
            restricted, and adding a restricted user as a buddy clears their verdict.
          </p>
          {#each filterLevels as [id, label, description]}
            <div class="form-row">
              <label>
                <input type="radio" name="filter-level" value={id} bind:group={draft.filter_level} />
                {label}
              </label>
              <span class="hint">{description}</span>
            </div>
          {/each}
          <div class="form-row">
            <label for="set-denied">Denial message</label>
            <input id="set-denied" style="width: 100%" bind:value={draft.denied_message} />
            <span class="hint">Sent to denied peers when they try to queue a download.</span>
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

        {:else if page === 'profile'}
          <h3>User Profile</h3>
          <p class="hint">Shown to users who view your profile.</p>
          <textarea rows="10" bind:value={draft.description}></textarea>

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
