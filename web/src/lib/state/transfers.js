import { writable, derived, get } from 'svelte/store';

import { actEach } from '../act.js';
import { post } from '../api.js';
import { plural } from '../format.js';
import { go } from '../ui.js';
import { report } from './session.js';

export const downloads = writable({});
export const uploads = writable({});

export const isCleared = (s) => s === 'finished' || s === 'aborted' || s === 'failed';

export const transferKey = (username, virtualPath) => `${username}\u0000${virtualPath}`;

export const speedTotals = derived([downloads, uploads], ([$downloads, $uploads]) => ({
	down: Object.values($downloads).reduce((n, t) => n + t.speed_bps, 0),
	up: Object.values($uploads).reduce((n, t) => n + t.speed_bps, 0),
}));

export const downloadIndex = derived(downloads, ($downloads) => {
	const index = new Map();
	for (const transfer of Object.values($downloads)) {
		index.set(transferKey(transfer.username, transfer.virtual_path), transfer);
	}
	return index;
});

export async function queueDownloads(files) {
	const index = get(downloadIndex);
	const fresh = files.filter((file) => {
		const transfer = index.get(transferKey(file.username, file.virtual_path));
		return !transfer || isCleared(transfer.status);
	});
	const result = await actEach(
		fresh.map((file) => post('/downloads', file)),
		{ failed: (count) => `${plural(count, 'download')} not queued`, skip: [409] },
	);
	if (!result.ok) return;
	const skipped = files.length - result.done;
	if (!result.done) {
		report.info(files.length > 1 ? 'Those files are already downloading.' : 'That file is already downloading.');
	} else if (files.length > 1) {
		report.done(
			`Queued ${plural(result.done, 'file')}` + (skipped ? ` (${skipped} already in Downloads)` : ''),
			{ label: 'Show Downloads', run: () => go('downloads') },
		);
	}
}

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
