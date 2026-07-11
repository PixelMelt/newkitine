import { get } from 'svelte/store';
import { buddies, banned, ignored } from './stores.js';
import { post, del } from './api.js';
import { openPrivateChat, openBrowse, openUserInfo } from './ui.js';

export function userMenu(username, { skip = [] } = {}) {
  const isBuddy = username in get(buddies);
  const isBanned = get(banned).includes(username);
  const isIgnored = get(ignored).includes(username);
  const encoded = encodeURIComponent(username);
  const items = [
    { label: username, action: () => navigator.clipboard.writeText(username) },
    { sep: true },
  ];
  if (!skip.includes('userinfo')) {
    items.push({ label: 'View User Profile', action: () => openUserInfo(username) });
  }
  if (!skip.includes('privatechat')) {
    items.push({ label: 'Send Message', action: () => openPrivateChat(username) });
  }
  if (!skip.includes('userbrowse')) {
    items.push({ label: 'Browse Files', action: () => openBrowse(username) });
  }
  if (!skip.includes('userlist')) {
    items.push({
      label: 'Add Buddy',
      checked: isBuddy,
      action: () => (isBuddy ? del(`/buddies/${encoded}`) : post('/buddies', { username })),
    });
  }
  items.push(
    { sep: true },
    {
      label: 'Ban User',
      checked: isBanned,
      action: () => (isBanned ? del(`/banned/${encoded}`) : post('/banned', { username })),
    },
    {
      label: 'Ignore User',
      checked: isIgnored,
      action: () => (isIgnored ? del(`/ignored/${encoded}`) : post('/ignored', { username })),
    },
  );
  return items;
}
