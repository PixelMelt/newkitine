import { writable } from 'svelte/store';

import { act } from '../act.js';
import { post } from '../api.js';
import { baseName, folderName, fold } from '../format.js';
import { go } from '../ui.js';

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

const scopes = new Map();
export const pendingTokens = new Set();
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
			scopes.set(token, scope);
			pendingTokens.add(String(token));
			go('search', String(token));
		},
	});
}

export const openSearch = (query) => startSearch({ query, mode: 'global' });

export const searchAgain = (search) =>
	startSearch(scopes.get(search.token) ?? { query: search.query, mode: 'global' });

export function applySnapshot(msg) {
	searches.set(msg.searches.map(viewOf));
	wishlist.set(msg.wishlist);
	interests.set(msg.interests);
}

export const handlers = {
	search_added: (msg) => {
		pendingTokens.delete(String(msg.search.token));
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
		scopes.delete(msg.token);
		searches.update((list) => list.filter((s) => s.token !== msg.token));
	},
	wishlist: (msg) => wishlist.set(msg.wishlist),
	interests: (msg) => interests.set(msg.interests),
};
