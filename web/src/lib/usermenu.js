import { get } from 'svelte/store';
import { buddies, banned, ignored, uploads, isCleared } from './stores.js';
import { post, del } from './api.js';
import { act, copy } from './act.js';
import { plural } from './format.js';
import { openPrivateChat, openBrowse, openUserInfo } from './ui.js';

const encoded = (username) => encodeURIComponent(username);

export function addBuddy(username, note = '') {
	return act(() => post('/buddies', { username }), {
		failed: `${username} not added to buddies`,
		done: `Added ${username} to buddies`,
		then: () => note && post(`/buddies/${encoded(username)}/note`, { note }),
		action: { label: 'Undo', run: () => removeBuddy(username) },
	});
}

export function removeBuddy(username) {
	const note = get(buddies).get(username)?.note ?? '';
	return act(() => del(`/buddies/${encoded(username)}`), {
		failed: `${username} not removed from buddies`,
		done: `Removed ${username} from buddies`,
		action: { label: 'Undo', run: () => addBuddy(username, note) },
	});
}

export function ban(username) {
	const denied = Object.values(get(uploads)).filter(
		(t) => t.username === username && !isCleared(t.status),
	).length;
	return act(() => post('/banned', { username }), {
		failed: `${username} not banned`,
		done: denied ? `Banned ${username} and denied ${plural(denied, 'upload')}` : `Banned ${username}`,
		action: { label: 'Unban', run: () => unban(username) },
	});
}

export function unban(username) {
	return act(() => del(`/banned/${encoded(username)}`), {
		failed: `${username} still banned`,
		done: `Unbanned ${username}`,
		action: { label: 'Ban Again', run: () => ban(username) },
	});
}

export function ignore(username) {
	return act(() => post('/ignored', { username }), {
		failed: `${username} not ignored`,
		done: `Ignoring ${username}`,
		action: { label: 'Undo', run: () => unignore(username) },
	});
}

export function unignore(username) {
	return act(() => del(`/ignored/${encoded(username)}`), {
		failed: `${username} still ignored`,
		done: `Stopped ignoring ${username}`,
		action: { label: 'Undo', run: () => ignore(username) },
	});
}

function banIp(username) {
	return act(() => post(`/users/${encoded(username)}/ban_ip`), {
		failed: `IP address of ${username} not banned`,
		done: `Banned the last known IP address of ${username}`,
		on: { 404: `no IP address is known for ${username}` },
	});
}

export function userMenu(username, { current = null, primary = false } = {}) {
	const isBuddy = get(buddies).has(username);
	const isBanned = get(banned).includes(username);
	const isIgnored = get(ignored).includes(username);
	return [
		{ title: username },
		{ label: 'Copy Username', action: () => copy(username, 'username') },
		{ sep: true },
		{
			label: 'View User Profile', primary,
			disabled: current === 'profile', action: () => openUserInfo(username),
		},
		{ label: 'Send Message', disabled: current === 'chat', action: () => openPrivateChat(username) },
		{ label: 'Browse Files', disabled: current === 'browse', action: () => openBrowse(username) },
		{ sep: true },
		isBuddy
			? { label: 'Remove Buddy', action: () => removeBuddy(username) }
			: { label: 'Add Buddy', action: () => addBuddy(username) },
		isIgnored
			? { label: 'Stop Ignoring User', action: () => unignore(username) }
			: { label: 'Ignore User', action: () => ignore(username) },
		isBanned
			? { label: 'Unban User', action: () => unban(username) }
			: { label: 'Ban User', action: () => ban(username) },
		{ label: 'Ban IP Address', action: () => banIp(username) },
	];
}
