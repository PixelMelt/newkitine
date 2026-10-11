import { writable } from 'svelte/store';

import * as chat from './chat.js';
import { validate } from './contract.js';
import * as searches from './searches.js';
import * as session from './session.js';
import * as transfers from './transfers.js';
import * as users from './users.js';

const RETRY_MS = 3000;
const LOST_AFTER_MS = 1000;
const STABLE_MS = 30000;

const domains = [session, transfers, searches, chat, users];

const handlers = Object.assign({}, ...domains.map((domain) => domain.handlers));

export const link = writable({ state: 'connecting', known: false, error: null });

let current = { state: 'connecting', known: false, error: null };
let lastRev = 0;
let retryTimer = null;
let lostTimer = null;
let violationAt = 0;
let socket = null;

function setLink(next) {
	current = next;
	link.set(next);
}

function dispatch(msg) {
	validate(msg);
	if (msg.type === 'snapshot') {
		lastRev = msg.rev;
		for (const domain of domains) {
			domain.applySnapshot(msg);
		}
		clearTimeout(lostTimer);
		lostTimer = null;
		setLink({ state: 'live', known: true, error: null });
		return;
	}
	const handler = handlers[msg.type];
	if (!handler) {
		throw new Error(`unhandled event type ${msg.type}`);
	}
	if (msg.rev <= lastRev) return;
	if (msg.rev !== lastRev + 1) {
		throw new Error(`revision gap: expected ${lastRev + 1}, got ${msg.rev}`);
	}
	lastRev = msg.rev;
	handler(msg);
}

export function connectWebSocket() {
	clearTimeout(retryTimer);
	if (socket) {
		socket.onmessage = null;
		socket.onclose = null;
		socket.close();
	}
	const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
	socket = new WebSocket(`${protocol}://${location.host}/api/ws`);
	socket.onmessage = (event) => {
		try {
			dispatch(JSON.parse(event.data));
		} catch (error) {
			console.error('websocket contract violation, resnapshotting', error);
			const repeated = Date.now() - violationAt < STABLE_MS;
			violationAt = Date.now();
			if (repeated) setLink({ state: 'broken', known: current.known, error: error.message });
			socket.close();
			throw error;
		}
	};
	socket.onclose = () => {
		if (current.state === 'broken') return;
		if (current.state !== 'lost' && lostTimer === null) {
			lostTimer = setTimeout(() => {
				lostTimer = null;
				setLink({ state: 'lost', known: current.known, error: null });
			}, LOST_AFTER_MS);
		}
		retryTimer = setTimeout(connectWebSocket, RETRY_MS);
	};
}
