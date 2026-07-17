import { writable } from 'svelte/store';
import { get as apiGet } from '../api.js';

export const rooms = writable({ available: [], joined: {} });
export const privateChats = writable({});
export const chatPartners = writable([]);

function messageKey(m) {
	return m.id != null ? `#${m.id}` : `${m.timestamp}\u0000${m.sender}\u0000${m.message}`;
}

function mergeHistory(history, live) {
	const seen = new Set(history.map(messageKey));
	const raced = live.filter((m) => m.status || !seen.has(messageKey(m)));
	return [...history, ...raced];
}

async function loadRoomHistory(room) {
	const data = await apiGet(`/rooms/${encodeURIComponent(room)}/messages?limit=100`);
	rooms.update((r) => {
		if (r.joined[room]) {
			r.joined[room].messages = mergeHistory(data.messages, r.joined[room].messages);
		}
		return r;
	});
}

export async function loadChatHistory(username) {
	const data = await apiGet(`/chats/${encodeURIComponent(username)}?limit=200`);
	privateChats.update((chats) => ({
		...chats,
		[username]: mergeHistory(data.messages, chats[username] ?? []),
	}));
}

export function applySnapshot(msg) {
	for (const room of Object.keys(msg.rooms.joined)) {
		msg.rooms.joined[room].messages = [];
	}
	rooms.set(msg.rooms);
	chatPartners.set(msg.chat_partners);
	for (const room of Object.keys(msg.rooms.joined)) {
		loadRoomHistory(room);
	}
	privateChats.update((chats) => {
		for (const username of Object.keys(chats)) {
			loadChatHistory(username);
		}
		return chats;
	});
}

export const handlers = {
	private_message: (msg) => {
		privateChats.update((chats) => ({
			...chats,
			[msg.username]: [...(chats[msg.username] ?? []), msg.message],
		}));
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
			const room = r.joined[msg.room];
			room.messages = [...room.messages.slice(-199), msg.message];
			return r;
		});
	},
	room_list: (msg) => {
		rooms.update((r) => ({ ...r, available: msg.rooms }));
	},
	room_joined: (msg) => {
		rooms.update((r) => {
			r.joined = { ...r.joined, [msg.room]: { users: msg.users, messages: [] } };
			return r;
		});
		loadRoomHistory(msg.room);
	},
	room_left: (msg) => {
		rooms.update((r) => {
			const joined = { ...r.joined };
			delete joined[msg.room];
			return { ...r, joined };
		});
	},
	room_user_joined: (msg) => {
		rooms.update((r) => {
			const room = r.joined[msg.room];
			if (room && !room.users.includes(msg.username)) {
				room.users = [...room.users, msg.username].sort();
			}
			return r;
		});
	},
	room_user_left: (msg) => {
		rooms.update((r) => {
			const room = r.joined[msg.room];
			if (room) {
				room.users = room.users.filter((u) => u !== msg.username);
			}
			return r;
		});
	},
};
