import { writable } from 'svelte/store';

export const activeTab = writable('search');
export const chatTarget = writable(null);
export const browseTarget = writable(null);
export const userInfoTarget = writable(null);
export const searchTarget = writable(null);

export function openPrivateChat(username) {
	chatTarget.set(username);
	activeTab.set('chat');
}

export function openBrowse(username, folder = null) {
	browseTarget.set({ username, folder });
	activeTab.set('browse');
}

export function openUserInfo(username) {
	userInfoTarget.set(username);
	activeTab.set('userinfo');
}

export function openSearch(query) {
	searchTarget.set(query);
	activeTab.set('search');
}
