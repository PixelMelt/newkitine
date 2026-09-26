import { writable } from 'svelte/store';
import { get as apiGet } from '../api.js';

export const rooms = writable({ available: [], joined: new Map() });
export const privateChats = writable(new Map());
export const chatPartners = writable([]);

const ROOM_MESSAGE_LIMIT = 200;
const historyRequested = new Set();

function mergeById(...lists) {
	const byId = new Map();
	for (const list of lists) {
		for (const message of list) byId.set(message.id, message);
	}
	return [...byId.values()].sort((a, b) => a.id - b.id);
}

async function loadRoomHistory(name) {
	const data = await apiGet(`/rooms/${encodeURIComponent(name)}/messages?limit=100`);
	rooms.update((r) => {
		const room = r.joined.get(name);
		if (room) {
			room.messages = mergeById(data.messages, room.messages);
		}
		return r;
	});
}

async function loadChatHistory(username) {
	const data = await apiGet(`/chats/${encodeURIComponent(username)}?limit=200`);
	privateChats.update((chats) =>
		chats.set(username, mergeById(chats.get(username) ?? [], data.messages)),
	);
}

export function ensureChatHistory(username) {
	if (historyRequested.has(username)) return;
	historyRequested.add(username);
	loadChatHistory(username);
}

export function applySnapshot(msg) {
	const joined = new Map(
		Object.entries(msg.rooms.joined).map(([name, view]) => [
			name,
			{ users: view.users, messages: [] },
		]),
	);
	rooms.set({ available: msg.rooms.available, joined });
	chatPartners.set(msg.chat_partners);
	for (const name of joined.keys()) {
		loadRoomHistory(name);
	}
	for (const username of historyRequested) {
		loadChatHistory(username);
	}
}

export const handlers = {
	private_message: (msg) => {
		privateChats.update((chats) =>
			chats.set(msg.username, mergeById(chats.get(msg.username) ?? [], [msg.message])),
		);
		chatPartners.update((list) =>
			list.includes(msg.username) ? list : [msg.username, ...list],
		);
	},
	chat_opened: (msg) => {
		chatPartners.update((list) =>
			list.includes(msg.username) ? list : [msg.username, ...list],
		);
	},
	chat_closed: (msg) => {
		chatPartners.update((list) => list.filter((user) => user !== msg.username));
	},
	room_message: (msg) => {
		rooms.update((r) => {
			const room = r.joined.get(msg.room);
			room.messages = mergeById(room.messages, [msg.message]).slice(-ROOM_MESSAGE_LIMIT);
			return r;
		});
	},
	room_list: (msg) => {
		rooms.update((r) => ({ ...r, available: msg.rooms }));
	},
	room_joined: (msg) => {
		rooms.update((r) => {
			r.joined.set(msg.room, { users: msg.users, messages: [] });
			return r;
		});
		loadRoomHistory(msg.room);
	},
	room_left: (msg) => {
		rooms.update((r) => {
			r.joined.delete(msg.room);
			return r;
		});
	},
	room_user_joined: (msg) => {
		rooms.update((r) => {
			const room = r.joined.get(msg.room);
			if (!room.users.includes(msg.username)) {
				room.users = [...room.users, msg.username].sort();
			}
			return r;
		});
	},
	room_user_left: (msg) => {
		rooms.update((r) => {
			const room = r.joined.get(msg.room);
			room.users = room.users.filter((u) => u !== msg.username);
			return r;
		});
	},
};
