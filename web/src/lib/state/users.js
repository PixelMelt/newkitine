import { writable } from 'svelte/store';

import { act } from '../act.js';
import { get as apiGet } from '../api.js';
import { report } from './session.js';

export const buddies = writable(new Map());
export const banned = writable([]);
export const ignored = writable([]);
export const userInfos = writable(new Map());
export const browses = writable(new Map());
export const ipBans = writable([]);
export const browseFailure = writable(null);
export const browseLoad = writable(null);

export const loadIpBans = () =>
	act(() => apiGet('/ip_bans'), {
		failed: 'Banned IP addresses not loaded',
		then: (data) => ipBans.set(data.patterns),
	});

function removeKey(key) {
	return (map) => {
		map.delete(key);
		return map;
	};
}

export function applySnapshot(msg) {
	buddies.set(new Map(msg.buddies.map((b) => [b.username, b])));
	banned.set(msg.banned);
	ignored.set(msg.ignored);
	browses.set(new Map(Object.entries(msg.browses)));
	userInfos.set(new Map(msg.user_infos.map((info) => [info.username, info])));
}

export const handlers = {
	buddy: (msg) => {
		buddies.update((map) => map.set(msg.buddy.username, msg.buddy));
	},
	buddy_removed: (msg) => buddies.update(removeKey(msg.username)),
	banned: (msg) => banned.set(msg.users),
	ignored: (msg) => ignored.set(msg.users),
	user_info: (msg) => {
		userInfos.update((map) => map.set(msg.info.username, msg.info));
	},
	user_info_removed: (msg) => userInfos.update(removeKey(msg.username)),
	browse_loaded: (msg) => {
		browses.update((map) => map.set(msg.username, msg.received_at));
		browseLoad.set({ username: msg.username });
	},
	browse_removed: (msg) => browses.update(removeKey(msg.username)),
	browse_failed: (msg) => browseFailure.set({ username: msg.username }),
	folder_request_failed: (msg) => {
		report.error(`${msg.username} did not send the contents of "${msg.directory}"`);
	},
};
