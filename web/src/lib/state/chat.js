import { writable, derived, get } from 'svelte/store';

import { act } from '../act.js';
import { get as apiGet, post, del } from '../api.js';
import { persisted, isRecord } from '../persisted.js';
import { status } from './session.js';

const ROOM_MESSAGE_LIMIT = 200;
const ROOM_HISTORY = 100;
const CHAT_HISTORY = 200;

export const rooms = writable({ available: [], joined: new Map() });
export const chatPartners = writable([]);
export const conversations = writable(new Map());
export const drafts = persisted('drafts', {}, isRecord);
export const readMarks = persisted('readMarks', {}, isRecord);

export const roomKey = (name) => `room:${name}`;
export const chatKey = (username) => `chat:${username}`;

export const unread = derived(
	[conversations, readMarks, status],
	([$conversations, $readMarks, $status]) => {
		const result = new Map();
		const self = $status.username.toLowerCase();
		for (const [key, messages] of $conversations) {
			if (!(key in $readMarks)) continue;
			const fresh = messages.filter(
				(m) => m.id > $readMarks[key] && m.sender !== $status.username,
			);
			if (!fresh.length) continue;
			result.set(key, {
				count: fresh.length,
				mention: self !== '' && fresh.some((m) => m.message.toLowerCase().includes(self)),
			});
		}
		return result;
	},
);

function merge(key, incoming, limit) {
	conversations.update((map) => {
		const byId = new Map();
		for (const message of map.get(key) ?? []) byId.set(message.id, message);
		for (const message of incoming) byId.set(message.id, message);
		const merged = [...byId.values()].sort((a, b) => a.id - b.id);
		return map.set(key, limit ? merged.slice(-limit) : merged);
	});
}

const firstVisit = Object.keys(get(readMarks)).length === 0;

function seedReadMark(key, messages, read) {
	readMarks.update((marks) =>
		key in marks ? marks : { ...marks, [key]: read && messages.length ? messages.at(-1).id : 0 },
	);
}

export function markRead(key) {
	const messages = get(conversations).get(key);
	if (!messages?.length) return;
	const last = messages.at(-1).id;
	if (get(readMarks)[key] !== last) readMarks.update((marks) => ({ ...marks, [key]: last }));
}

async function loadRoomHistory(name) {
	const data = await apiGet(`/rooms/${encodeURIComponent(name)}/messages?limit=${ROOM_HISTORY}`);
	seedReadMark(roomKey(name), data.messages, true);
	merge(roomKey(name), data.messages, ROOM_MESSAGE_LIMIT);
}

async function loadChatHistory(username, read = true) {
	const data = await apiGet(`/chats/${encodeURIComponent(username)}?limit=${CHAT_HISTORY}`);
	seedReadMark(chatKey(username), data.messages, read);
	merge(chatKey(username), data.messages);
}

function addPartner(username) {
	let added = false;
	chatPartners.update((list) => {
		added = !list.includes(username);
		return added ? [...list, username] : list;
	});
	return added;
}

export function openChat(username) {
	if (!addPartner(username)) return;
	act(() => post(`/chats/${encodeURIComponent(username)}/open`), {
		failed: `Chat with ${username} not opened`,
		then: () => loadChatHistory(username),
	}).then((opened) => opened || forgetChat(username));
}

function dropConversation(key) {
	conversations.update((map) => {
		map.delete(key);
		return map;
	});
}

export function forgetChat(username) {
	chatPartners.update((list) => list.filter((user) => user !== username));
	dropConversation(chatKey(username));
}

export function closeChat(username) {
	return act(() => del(`/chats/${encodeURIComponent(username)}`), {
		failed: `Chat with ${username} not closed`,
		then: () => forgetChat(username),
		done: `Closed chat with ${username}`,
		action: { label: 'Undo', run: () => openChat(username) },
	});
}

export function joinRoom(room) {
	return act(() => post('/rooms/join', { room }), { failed: `Room ${room} not joined` });
}

export function leaveRoom(room) {
	return act(() => post('/rooms/leave', { room }), {
		failed: `Still in ${room}`,
		done: `Left ${room}`,
		action: { label: 'Rejoin', run: () => joinRoom(room) },
	});
}

export function sendMessage(key, where) {
	const text = (get(drafts)[key] ?? '').trim();
	if (!text) return Promise.resolve(false);
	const [kind, target] = [key.slice(0, key.indexOf(':')), key.slice(key.indexOf(':') + 1)];
	const path =
		kind === 'room'
			? `/rooms/${encodeURIComponent(target)}/messages`
			: `/chats/${encodeURIComponent(target)}`;
	return act(() => post(path, { message: text }), {
		failed: 'Message not sent',
		where,
		then: () =>
			drafts.update((all) => {
				if ((all[key] ?? '').trim() !== text) return all;
				const rest = { ...all };
				delete rest[key];
				return rest;
			}),
	});
}

export function applySnapshot(msg) {
	const joined = new Map(
		Object.entries(msg.rooms.joined).map(([name, view]) => [name, { users: view.users }]),
	);
	rooms.set({ available: msg.rooms.available, joined });
	chatPartners.set(msg.chat_partners);
	for (const name of joined.keys()) loadRoomHistory(name);
	for (const username of msg.chat_partners) loadChatHistory(username, firstVisit);
}

export const handlers = {
	private_message: (msg) => {
		const key = chatKey(msg.username);
		if (addPartner(msg.username)) {
			readMarks.update((marks) => (key in marks ? marks : { ...marks, [key]: 0 }));
			loadChatHistory(msg.username);
		}
		merge(key, [msg.message]);
	},
	chat_opened: (msg) => {
		if (addPartner(msg.username)) loadChatHistory(msg.username);
	},
	chat_closed: (msg) => forgetChat(msg.username),
	room_message: (msg) => merge(roomKey(msg.room), [msg.message], ROOM_MESSAGE_LIMIT),
	room_list: (msg) => {
		rooms.update((r) => ({ ...r, available: msg.rooms }));
	},
	room_joined: (msg) => {
		rooms.update((r) => {
			r.joined.set(msg.room, { users: msg.users });
			return r;
		});
		loadRoomHistory(msg.room);
	},
	room_left: (msg) => {
		rooms.update((r) => {
			r.joined.delete(msg.room);
			return r;
		});
		dropConversation(roomKey(msg.room));
	},
	room_user_joined: (msg) => {
		rooms.update((r) => {
			const room = r.joined.get(msg.room);
			if (!room.users.includes(msg.username)) room.users = [...room.users, msg.username];
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
