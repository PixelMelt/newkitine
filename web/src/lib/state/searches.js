import { writable, get } from 'svelte/store';

import { act } from '../act.js';
import { post } from '../api.js';
import { baseName, folderName, fold } from '../format.js';
import { go } from '../ui.js';
import { persisted, isRecord } from '../persisted.js';

export const searches = writable([]);
export const wishlist = writable([]);
export const interests = writable({
	liked: [],
	hated: [],
	recommendations: [],
	unrecommendations: [],
	recommendations_for: null,
	recommendations_global: false,
	similar_users: [],
	similar_users_for: null,
});

const scopes = persisted('searchScopes', {}, isRecord);
let nextRowKey = 1;

function rowsOf(response) {
	return response.files.map((file) => ({
		key: nextRowKey++,
		response,
		file,
		folder: folderName(file.name),
		filename: baseName(file.name),
		folded: fold(file.name),
	}));
}

const viewOf = (search) => ({
	token: search.token,
	query: search.query,
	rows: search.results.flatMap(rowsOf),
});

export function startSearch(scope) {
	return act(() => post('/searches', scope), {
		failed: `Search for "${scope.query}" not started`,
		then: ({ token }) => {
			scopes.update((known) => ({ ...known, [token]: scope }));
			go('search', String(token));
		},
	});
}

export const openSearch = (query) => startSearch({ query, mode: 'global' });

export const scopeOf = (search) =>
	get(scopes)[search.token] ?? { query: search.query, mode: 'global' };

export function applySnapshot(msg) {
	searches.set(msg.searches.map(viewOf));
	scopes.update((known) =>
		Object.fromEntries(msg.searches.filter((s) => s.token in known).map((s) => [s.token, known[s.token]])),
	);
	wishlist.set(msg.wishlist);
	interests.set(msg.interests);
}

export const handlers = {
	search_added: (msg) => {
		searches.update((list) =>
			list.some((s) => s.token === msg.search.token) ? list : [...list, viewOf(msg.search)],
		);
	},
	search_results: (msg) => {
		searches.update((list) => {
			const search = list.find((s) => s.token === msg.token);
			if (search) search.rows = [...search.rows, ...rowsOf(msg.response)];
			return list;
		});
	},
	search_removed: (msg) => {
		scopes.update(({ [msg.token]: _closed, ...rest }) => rest);
		searches.update((list) => list.filter((s) => s.token !== msg.token));
	},
	wishlist: (msg) => wishlist.set(msg.wishlist),
	interests: (msg) => interests.set(msg.interests),
};
