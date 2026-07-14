function fail(path, expected) {
	return `${path}: expected ${expected}`;
}

const str = (v, path) => (typeof v === 'string' ? null : fail(path, 'string'));
const num = (v, path) => (typeof v === 'number' ? null : fail(path, 'number'));
const bool = (v, path) => (typeof v === 'boolean' ? null : fail(path, 'boolean'));
const nullable = (inner) => (v, path) => (v === null ? null : inner(v, path));

const list = (inner) => (v, path) => {
	if (!Array.isArray(v)) return fail(path, 'array');
	for (let i = 0; i < v.length; i++) {
		const err = inner(v[i], `${path}[${i}]`);
		if (err) return err;
	}
	return null;
};

const record = (inner) => (v, path) => {
	if (v === null || typeof v !== 'object' || Array.isArray(v)) return fail(path, 'object');
	for (const [key, value] of Object.entries(v)) {
		const err = inner(value, `${path}.${key}`);
		if (err) return err;
	}
	return null;
};

const shape = (fields) => (v, path) => {
	if (v === null || typeof v !== 'object' || Array.isArray(v)) return fail(path, 'object');
	for (const [key, inner] of Object.entries(fields)) {
		const err = inner(v[key], `${path}.${key}`);
		if (err) return err;
	}
	return null;
};

const oneOf = (...values) => (v, path) =>
	values.includes(v) ? null : fail(path, values.join('|'));

const pair = (a, b) => (v, path) => {
	if (!Array.isArray(v) || v.length !== 2) return fail(path, 'pair');
	return a(v[0], `${path}[0]`) ?? b(v[1], `${path}[1]`);
};

const direction = oneOf('download', 'upload');

const userStats = shape({
	avgspeed: num,
	uploadnum: num,
	unknown: num,
	files: num,
	dirs: num,
});

const fileAttributes = shape({
	bitrate: nullable(num),
	length: nullable(num),
	vbr: nullable(num),
	sample_rate: nullable(num),
	bit_depth: nullable(num),
});

const status = shape({
	connected: bool,
	logged_in: bool,
	username: str,
	server: str,
	banner: str,
	listen_port: num,
	shared_folders: num,
	shared_files: num,
	scanning: bool,
	scan_progress: num,
	share_scan_error: nullable(str),
	privileges_secs: num,
	peer_connections: num,
});

const transferView = shape({
	id: num,
	username: str,
	virtual_path: str,
	size: num,
	bytes_done: num,
	status: oneOf('queued', 'transferring', 'finished', 'aborted', 'failed'),
	failure_reason: nullable(str),
	file_path: nullable(str),
	queue_place: num,
	speed_bps: num,
	attributes: fileAttributes,
	updated_at: num,
});

const searchFileView = shape({
	name: str,
	size: num,
	attributes: fileAttributes,
});

const searchResponseView = shape({
	username: str,
	free_upload_slots: bool,
	upload_speed: num,
	queue_size: num,
	files: list(searchFileView),
});

const searchView = shape({
	token: num,
	query: str,
	results: list(searchResponseView),
});

const buddyView = shape({
	username: str,
	status: str,
	privileged: bool,
	stats: userStats,
	note: str,
});

const userInfoView = shape({
	username: str,
	received: bool,
	description: str,
	picture_base64: nullable(str),
	upload_slots: num,
	queue_size: num,
	slots_available: bool,
	stats: nullable(userStats),
	interests_liked: list(str),
	interests_hated: list(str),
});

const chatMessage = shape({
	sender: str,
	message: str,
	timestamp: num,
});

const roomEntry = shape({
	name: str,
	users: num,
});

const roomView = shape({
	users: list(str),
	messages: list(chatMessage),
});

const roomsView = shape({
	available: list(roomEntry),
	joined: record(roomView),
});

const recommendations = list(pair(str, num));

const interestsView = shape({
	liked: list(str),
	hated: list(str),
	recommendations,
	unrecommendations: recommendations,
	recommendations_for: nullable(str),
	recommendations_global: bool,
	similar_users: list(shape({ username: str, rating: num })),
	similar_users_for: nullable(str),
});

const publicSettings = shape({
	server: str,
	username: str,
	password_set: bool,
	listen_port: num,
	description: str,
	download_dir: str,
	incomplete_dir: nullable(str),
	shares: list(shape({ virtual_name: str, path: str, buddy_only: bool })),
	upload_slots: num,
	queue_file_limit: num,
	uploads_per_user: num,
	upload_limit_kbps: num,
	download_limit_kbps: num,
	auto_reconnect: bool,
	theme: str,
	filter_level: str,
	denied_message: str,
});

const settingsPayload = {
	settings: publicSettings,
	locked: list(str),
	gluetun: bool,
};

const events = {
	status: shape({ rev: num, status }),
	conn_count: shape({ rev: num, count: num }),
	login_failed: shape({ rev: num, reason: str, detail: nullable(str) }),
	server_message: shape({ rev: num, message: str }),
	settings: shape({ rev: num, ...settingsPayload }),
	transfer: shape({ rev: num, direction, transfer: transferView }),
	transfers_removed: shape({ rev: num, direction, ids: list(num) }),
	search_added: shape({ rev: num, search: searchView }),
	search_results: shape({ rev: num, token: num, response: searchResponseView }),
	search_removed: shape({ rev: num, token: num }),
	wishlist: shape({ rev: num, wishlist: list(str) }),
	interests: shape({ rev: num, interests: interestsView }),
	buddy: shape({ rev: num, buddy: buddyView }),
	buddy_removed: shape({ rev: num, username: str }),
	banned: shape({ rev: num, users: list(str) }),
	ignored: shape({ rev: num, users: list(str) }),
	user_info: shape({ rev: num, info: userInfoView }),
	browse_loaded: shape({ rev: num, username: str, received_at: num }),
	private_message: shape({ rev: num, username: str, message: chatMessage }),
	chat_opened: shape({ rev: num, username: str }),
	chat_closed: shape({ rev: num, username: str }),
	room_message: shape({ rev: num, room: str, message: chatMessage }),
	room_list: shape({ rev: num, rooms: list(roomEntry) }),
	room_joined: shape({ rev: num, room: str, users: list(str) }),
	room_left: shape({ rev: num, room: str }),
	room_user_joined: shape({ rev: num, room: str, username: str }),
	room_user_left: shape({ rev: num, room: str, username: str }),
	snapshot: shape({
		rev: num,
		status,
		downloads: list(transferView),
		uploads: list(transferView),
		searches: list(searchView),
		buddies: list(buddyView),
		banned: list(str),
		ignored: list(str),
		wishlist: list(str),
		interests: interestsView,
		rooms: roomsView,
		chat_partners: list(str),
		browses: record(num),
		settings: shape(settingsPayload),
	}),
};

export const eventTypes = Object.keys(events);

export function validate(msg) {
	const check = events[msg.type];
	if (!check) {
		throw new Error(`unknown event type ${msg.type}`);
	}
	const error = check(msg, msg.type);
	if (error) {
		throw new Error(`contract violation in ${msg.type} event: ${error}`);
	}
}
