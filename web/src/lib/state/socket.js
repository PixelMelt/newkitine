import * as chat from './chat.js';
import * as searches from './searches.js';
import * as session from './session.js';
import * as transfers from './transfers.js';
import * as users from './users.js';

const domains = [session, transfers, searches, chat, users];

const handlers = Object.assign({}, ...domains.map((domain) => domain.handlers));

let lastRev = 0;

function dispatch(msg) {
	if (msg.type === 'snapshot') {
		lastRev = msg.rev;
		for (const domain of domains) {
			domain.applySnapshot(msg);
		}
		return;
	}
	const handler = handlers[msg.type];
	if (!handler) {
		throw new Error(`unknown event type ${msg.type}`);
	}
	if (msg.rev <= lastRev) return;
	lastRev = msg.rev;
	handler(msg);
}

export function connectWebSocket() {
	const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
	const socket = new WebSocket(`${protocol}://${location.host}/api/ws`);
	socket.onmessage = (event) => {
		try {
			dispatch(JSON.parse(event.data));
		} catch (error) {
			console.error('websocket contract violation, resnapshotting', error);
			socket.close();
			throw error;
		}
	};
	socket.onclose = () => setTimeout(connectWebSocket, 3000);
}
