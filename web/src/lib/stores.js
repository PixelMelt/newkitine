import { writable, derived } from 'svelte/store';
import { get as apiGet } from './api.js';

export const status = writable({ connected: false, logged_in: false, username: '' });
export const searches = writable([]);
export const downloads = writable({});
export const uploads = writable({});
export const rooms = writable({ available: [], joined: {} });
export const privateChats = writable({});
export const chatPartners = writable([]);
export const buddies = writable({});
export const banned = writable([]);
export const ignored = writable([]);
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
export const userInfos = writable({});
export const browses = writable({});
export const notices = writable([]);
export const settings = writable({ settings: null, locked: [], gluetun: false });

export function applyTheme(theme) {
	document.documentElement.dataset.theme = theme;
	localStorage.setItem('theme', theme);
}

export const transferKey = (t) => t.username + '\n' + t.virtual_path;
export const isCleared = (s) => s === 'finished' || s === 'aborted' || s.startsWith('failed');

export const speedTotals = derived([downloads, uploads], ([$downloads, $uploads]) => ({
	down: Object.values($downloads).reduce((n, t) => n + (t.speed_bps ?? 0), 0),
	up: Object.values($uploads).reduce((n, t) => n + (t.speed_bps ?? 0), 0),
}));

const speedSamples = new Map();

function withSpeed(t) {
	const key = transferKey(t);
	if (t.status !== 'transferring') {
		speedSamples.delete(key);
		return { ...t, speed_bps: 0 };
	}
	const now = Date.now();
	const prev = speedSamples.get(key);
	let bps = prev?.bps ?? 0;
	if (prev && now > prev.at && t.bytes_done >= prev.bytes) {
		bps = ((t.bytes_done - prev.bytes) * 1000) / (now - prev.at);
	}
	speedSamples.set(key, { bytes: t.bytes_done, at: now, bps });
	return { ...t, speed_bps: bps };
}

function notice(text) {
	notices.update((list) => [...list.slice(-19), { text, at: Date.now() }]);
}

async function loadRoomHistory(room) {
	const data = await apiGet(`/rooms/${encodeURIComponent(room)}/messages?limit=100`);
	rooms.update((r) => {
		if (r.joined[room]) r.joined[room].messages = data.messages;
		return r;
	});
}

export async function loadChatHistory(username) {
	const data = await apiGet(`/chats/${encodeURIComponent(username)}?limit=200`);
	privateChats.update((chats) => ({ ...chats, [username]: data.messages }));
}

function dispatch(msg) {
	switch (msg.type) {
		case 'status':
			status.set(msg.status);
			break;
		case 'conn_count':
			status.update((s) => ({ ...s, peer_connections: msg.count }));
			break;
		case 'login_failed':
			notice(`login failed: ${msg.reason} ${msg.detail ?? ''}`);
			break;
		case 'server_message':
			notice(`server: ${msg.message}`);
			break;
		case 'search_added':
			searches.update((list) =>
				list.some((s) => s.token === msg.search.token) ? list : [...list, msg.search],
			);
			break;
		case 'search_results':
			searches.update((list) => {
				const search = list.find((s) => s.token === msg.token);
				if (search) search.results = [...search.results, msg.response];
				return list;
			});
			break;
		case 'search_removed':
			searches.update((list) => list.filter((s) => s.token !== msg.token));
			break;
		case 'transfer': {
			const store = msg.direction === 'download' ? downloads : uploads;
			const transfer = withSpeed(msg.transfer);
			store.update((map) => ({ ...map, [transferKey(transfer)]: transfer }));
			break;
		}
		case 'transfers_cleared': {
			const store = msg.direction === 'download' ? downloads : uploads;
			if (msg.scope === 'all') {
				store.set({});
				break;
			}
			const cleared = (status) =>
				msg.statuses.some((s) =>
					s === 'failed' ? status.startsWith('failed') : status === s,
				);
			store.update((map) => {
				const kept = {};
				for (const [key, t] of Object.entries(map)) {
					if (!cleared(t.status)) kept[key] = t;
				}
				return kept;
			});
			break;
		}
		case 'private_message':
			privateChats.update((chats) => ({
				...chats,
				[msg.username]: [...(chats[msg.username] ?? []), msg.message],
			}));
			chatPartners.update((list) =>
				list.includes(msg.username) ? list : [msg.username, ...list],
			);
			break;
		case 'chat_opened':
			chatPartners.update((list) =>
				list.includes(msg.username) ? list : [msg.username, ...list],
			);
			break;
		case 'chat_closed':
			chatPartners.update((list) => list.filter((user) => user !== msg.username));
			break;
		case 'room_message':
			rooms.update((r) => {
				const room = r.joined[msg.room];
				if (room) room.messages = [...room.messages.slice(-199), msg.message];
				return r;
			});
			break;
		case 'room_list':
			rooms.update((r) => ({ ...r, available: msg.rooms }));
			break;
		case 'room_joined':
			rooms.update((r) => {
				r.joined = { ...r.joined, [msg.room]: { users: msg.users, messages: [] } };
				return r;
			});
			loadRoomHistory(msg.room);
			break;
		case 'room_left':
			rooms.update((r) => {
				const joined = { ...r.joined };
				delete joined[msg.room];
				return { ...r, joined };
			});
			break;
		case 'room_user_joined':
			rooms.update((r) => {
				const room = r.joined[msg.room];
				if (room && !room.users.includes(msg.username)) {
					room.users = [...room.users, msg.username].sort();
				}
				return r;
			});
			break;
		case 'room_user_left':
			rooms.update((r) => {
				const room = r.joined[msg.room];
				if (room) {
					room.users = room.users.filter((u) => u !== msg.username);
				}
				return r;
			});
			break;
		case 'buddy':
			buddies.update((map) => ({ ...map, [msg.buddy.username]: msg.buddy }));
			break;
		case 'buddy_removed':
			buddies.update((map) => {
				const next = { ...map };
				delete next[msg.username];
				return next;
			});
			break;
		case 'user_status':
			buddies.update((map) => {
				const buddy = map[msg.username];
				if (buddy) {
					buddy.status = msg.status;
					buddy.privileged = msg.privileged;
					return { ...map };
				}
				return map;
			});
			break;
		case 'banned':
			banned.set(msg.users);
			break;
		case 'ignored':
			ignored.set(msg.users);
			break;
		case 'wishlist':
			wishlist.set(msg.wishlist);
			break;
		case 'interests':
			interests.set(msg.interests);
			break;
		case 'user_info':
			userInfos.update((map) => ({ ...map, [msg.info.username]: msg.info }));
			break;
		case 'browse_loaded':
			browses.update((map) => ({ ...map, [msg.username]: Date.now() }));
			break;
		case 'settings':
			settings.set({ settings: msg.settings, locked: msg.locked, gluetun: msg.gluetun });
			applyTheme(msg.settings.theme);
			break;
	}
}

export function connectWebSocket() {
	const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
	const socket = new WebSocket(`${protocol}://${location.host}/api/ws`);
	socket.onmessage = (event) => dispatch(JSON.parse(event.data));
	socket.onclose = () => setTimeout(connectWebSocket, 3000);
}

export async function loadInitialState() {
	const [
		statusData,
		searchList,
		downloadData,
		uploadData,
		buddyData,
		bannedData,
		ignoredData,
		wishlistData,
		interestsData,
		roomsData,
		chatsData,
		settingsData,
	] = await Promise.all([
		apiGet('/status'),
		apiGet('/searches'),
		apiGet('/downloads'),
		apiGet('/uploads'),
		apiGet('/buddies'),
		apiGet('/banned'),
		apiGet('/ignored'),
		apiGet('/wishlist'),
		apiGet('/interests'),
		apiGet('/rooms'),
		apiGet('/chats'),
		apiGet('/settings'),
	]);
	status.set(statusData.status);
	settings.set(settingsData);
	applyTheme(settingsData.settings.theme);
	downloads.set(Object.fromEntries(downloadData.transfers.map((t) => [transferKey(t), t])));
	uploads.set(Object.fromEntries(uploadData.transfers.map((t) => [transferKey(t), t])));
	buddies.set(Object.fromEntries(buddyData.buddies.map((b) => [b.username, b])));
	banned.set(bannedData.users);
	ignored.set(ignoredData.users);
	wishlist.set(wishlistData.wishlist);
	interests.set(interestsData.interests);
	rooms.set(roomsData.rooms);
	chatPartners.set(chatsData.chats);
	const full = await Promise.all(searchList.searches.map((s) => apiGet(`/searches/${s.token}`)));
	searches.set(full);
	for (const room of Object.keys(roomsData.rooms.joined)) {
		loadRoomHistory(room);
	}
}
