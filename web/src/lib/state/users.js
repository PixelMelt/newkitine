import { writable } from 'svelte/store';

export const buddies = writable({});
export const banned = writable([]);
export const ignored = writable([]);
export const userInfos = writable({});
export const browses = writable({});

export function applySnapshot(msg) {
	buddies.set(Object.fromEntries(msg.buddies.map((b) => [b.username, b])));
	banned.set(msg.banned);
	ignored.set(msg.ignored);
	browses.set(msg.browses);
	userInfos.set(Object.fromEntries(msg.user_infos.map((info) => [info.username, info])));
}

export const handlers = {
	buddy: (msg) => {
		buddies.update((map) => ({ ...map, [msg.buddy.username]: msg.buddy }));
	},
	buddy_removed: (msg) => {
		buddies.update((map) => {
			const next = { ...map };
			delete next[msg.username];
			return next;
		});
	},
	banned: (msg) => banned.set(msg.users),
	ignored: (msg) => ignored.set(msg.users),
	user_info: (msg) => {
		userInfos.update((map) => ({ ...map, [msg.info.username]: msg.info }));
	},
	user_info_removed: (msg) => {
		userInfos.update((map) => {
			const next = { ...map };
			delete next[msg.username];
			return next;
		});
	},
	browse_loaded: (msg) => {
		browses.update((map) => ({ ...map, [msg.username]: msg.received_at }));
	},
	browse_removed: (msg) => {
		browses.update((map) => {
			const next = { ...map };
			delete next[msg.username];
			return next;
		});
	},
};
