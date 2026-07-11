import { writable } from 'svelte/store';

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

export function applySnapshot(msg) {
	searches.set(msg.searches);
	wishlist.set(msg.wishlist);
	interests.set(msg.interests);
}

export const handlers = {
	search_added: (msg) => {
		searches.update((list) =>
			list.some((s) => s.token === msg.search.token) ? list : [...list, msg.search],
		);
	},
	search_results: (msg) => {
		searches.update((list) => {
			const search = list.find((s) => s.token === msg.token);
			if (search) search.results = [...search.results, msg.response];
			return list;
		});
	},
	search_removed: (msg) => {
		searches.update((list) => list.filter((s) => s.token !== msg.token));
	},
	wishlist: (msg) => wishlist.set(msg.wishlist),
	interests: (msg) => interests.set(msg.interests),
};
