import { get, writable } from 'svelte/store';

import { isRecord } from './persisted.js';
import { openChat } from './state/chat.js';

export const TABS = [
	'search', 'downloads', 'uploads', 'browse', 'user', 'chat', 'rooms',
	'interests', 'buddies', 'statistics', 'settings',
];

const STORAGE_KEY = 'route';

function stored() {
	const value = JSON.parse(localStorage.getItem(STORAGE_KEY));
	const valid =
		isRecord(value) && TABS.includes(value.tab) && isRecord(value.items) &&
		Object.values(value.items).every((item) => typeof item === 'string');
	return valid ? { tab: value.tab, items: value.items, folder: null } : null;
}

export function decodeSegment(segment) {
	try {
		return decodeURIComponent(segment);
	} catch {
		return '';
	}
}

function parse(pathname, previous) {
	const [tab, item, ...rest] = pathname.split('/').filter(Boolean).map(decodeSegment);
	if (!tab) return previous ?? { tab: 'search', items: {}, folder: null };
	if (!TABS.includes(tab)) return { tab: 'search', items: previous?.items ?? {}, folder: null };
	const items = { ...previous?.items };
	if (item) items[tab] = item;
	return { tab, items, folder: rest.length ? rest.join('\\') : null };
}

function pathOf({ tab, items, folder }) {
	const segments = [tab];
	if (items[tab]) segments.push(items[tab]);
	if (tab === 'browse' && items[tab] && folder) segments.push(...folder.split('\\'));
	return '/' + segments.map(encodeURIComponent).join('/');
}

export const route = writable(parse(location.pathname, stored()));

route.subscribe((value) =>
	localStorage.setItem(STORAGE_KEY, JSON.stringify({ tab: value.tab, items: value.items })),
);
history.replaceState(null, '', pathOf(get(route)));
window.addEventListener('popstate', () => route.set(parse(location.pathname, get(route))));

function commit(next, replace) {
	const path = pathOf(next);
	if (path !== location.pathname) {
		if (replace) history.replaceState(null, '', path);
		else history.pushState(null, '', path);
	}
	route.set(next);
}

export function go(tab, item, folder = null) {
	const current = get(route);
	const items = item === undefined ? current.items : { ...current.items, [tab]: item };
	const replace = tab === current.tab && items[tab] === current.items[tab];
	commit({ tab, items, folder }, replace);
}

export function remember(tab, item) {
	const current = get(route);
	commit({ ...current, items: { ...current.items, [tab]: item } }, true);
}

export function forget(tab, item) {
	const current = get(route);
	if (current.items[tab] !== item) return;
	const items = { ...current.items };
	delete items[tab];
	commit({ ...current, items }, true);
}

export const hrefOf = (current, tab) => pathOf({ ...current, tab, folder: null });

export function focusPane(tab) {
	const primary = [...document.querySelectorAll(`.pane[data-tab="${tab}"] [data-primary]`)]
		.find((node) => node.offsetParent !== null);
	if (primary) primary.focus();
}

export function openPrivateChat(username) {
	openChat(username);
	go('chat', username);
}
export const openBrowse = (username, folder = null) => go('browse', username, folder);
export const openUserInfo = (username) => go('user', username);

export const settingsDirty = writable(false);

export const noAutofill = {
	autocomplete: 'off',
	'data-1p-ignore': '',
	'data-lpignore': 'true',
	'data-bwignore': '',
	'data-form-type': 'other',
};
