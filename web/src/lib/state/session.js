import { writable } from 'svelte/store';

const EXPIRY_MS = { info: 6000, done: 6000, error: 20000 };
const ACTION_EXPIRY_MS = 10000;

const LOGIN_REASONS = {
	INVALIDPASS: 'wrong password',
	INVALIDUSERNAME: 'the server rejected the username',
};

export const status = writable({
	connected: false,
	logged_in: false,
	username: '',
	server: '',
	banner: '',
	listen_port: 0,
	shared_folders: 0,
	shared_files: 0,
	scanning: false,
	scan_progress: 0,
	share_scan_error: null,
	privileges_secs: 0,
	peer_connections: 0,
});
export const settings = writable({ settings: null, locked: [], gluetun: false });
export const loginFailure = writable(null);
export const shownReport = writable(null);

let expiry = null;

function push(kind, text, action) {
	shownReport.set({ kind, text, action });
	clearTimeout(expiry);
	expiry = setTimeout(dismissReport, action ? ACTION_EXPIRY_MS : EXPIRY_MS[kind]);
}

export function dismissReport() {
	clearTimeout(expiry);
	shownReport.set(null);
}

export const report = {
	error: (text, action) => push('error', text, action),
	done: (text, action) => push('done', text, action),
	info: (text) => push('info', text),
};

export function previewTheme(theme) {
	document.documentElement.dataset.theme = theme;
}

function applyStatus(next) {
	status.set(next);
	if (next.connected) loginFailure.set(null);
}

function applySettings(payload) {
	settings.set({ settings: payload.settings, locked: payload.locked, gluetun: payload.gluetun });
	if (localStorage.getItem('theme') !== payload.settings.theme) {
		previewTheme(payload.settings.theme);
		localStorage.setItem('theme', payload.settings.theme);
	}
}

export function applySnapshot(msg) {
	applyStatus(msg.status);
	applySettings(msg.settings);
}

export const handlers = {
	status: (msg) => applyStatus(msg.status),
	conn_count: (msg) => status.update((s) => ({ ...s, peer_connections: msg.count })),
	login_failed: (msg) => {
		const reason = LOGIN_REASONS[msg.reason] ?? msg.reason;
		loginFailure.set(msg.detail ? `${reason} (${msg.detail})` : reason);
	},
	server_message: (msg) => report.info(`Soulseek server: ${msg.message}`),
	relogged: () =>
		report.error('Signed out: this Soulseek account logged in from somewhere else'),
	settings: applySettings,
};
