import { writable } from 'svelte/store';

export const status = writable({
	connected: false,
	logged_in: false,
	username: '',
	server: '',
	banner: '',
	listen_port: 0,
	shared_folders: 0,
	shared_files: 0,
	share_scan_error: null,
	privileges_secs: 0,
	peer_connections: 0,
});
export const notices = writable([]);
export const settings = writable({ settings: null, locked: [], gluetun: false });

export function applyTheme(theme) {
	document.documentElement.dataset.theme = theme;
	localStorage.setItem('theme', theme);
}

export function notice(text) {
	notices.update((list) => [...list.slice(-19), { text, at: Date.now() }]);
}

export function applySnapshot(msg) {
	status.set(msg.status);
	settings.set(msg.settings);
	applyTheme(msg.settings.settings.theme);
}

export const handlers = {
	status: (msg) => status.set(msg.status),
	conn_count: (msg) => status.update((s) => ({ ...s, peer_connections: msg.count })),
	login_failed: (msg) => notice(`login failed: ${msg.reason} ${msg.detail ?? ''}`),
	server_message: (msg) => notice(`server: ${msg.message}`),
	settings: (msg) => {
		settings.set({ settings: msg.settings, locked: msg.locked, gluetun: msg.gluetun });
		applyTheme(msg.settings.theme);
	},
};
