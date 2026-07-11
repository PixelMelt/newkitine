import { writable, derived } from 'svelte/store';

export const downloads = writable({});
export const uploads = writable({});

export const isCleared = (s) => s === 'finished' || s === 'aborted' || s === 'failed';

export const speedTotals = derived([downloads, uploads], ([$downloads, $uploads]) => ({
	down: Object.values($downloads).reduce((n, t) => n + t.speed_bps, 0),
	up: Object.values($uploads).reduce((n, t) => n + t.speed_bps, 0),
}));

export function applySnapshot(msg) {
	downloads.set(Object.fromEntries(msg.downloads.map((t) => [t.id, t])));
	uploads.set(Object.fromEntries(msg.uploads.map((t) => [t.id, t])));
}

export const handlers = {
	transfer: (msg) => {
		const store = msg.direction === 'download' ? downloads : uploads;
		store.update((map) => ({ ...map, [msg.transfer.id]: msg.transfer }));
	},
	transfers_removed: (msg) => {
		const store = msg.direction === 'download' ? downloads : uploads;
		store.update((map) => {
			const kept = { ...map };
			for (const id of msg.ids) delete kept[id];
			return kept;
		});
	},
};
