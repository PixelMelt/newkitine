export function formatSize(bytes) {
	if (bytes >= 1024 ** 4) return (bytes / 1024 ** 4).toFixed(2) + ' TiB';
	if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(2) + ' GiB';
	if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + ' MiB';
	if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KiB';
	return bytes + ' B';
}

export function formatQuality(attributes) {
	if (attributes.sample_rate && attributes.bit_depth)
		return `${attributes.sample_rate / 1000} kHz / ${attributes.bit_depth} bit`;
	if (attributes.bitrate) return attributes.bitrate + ' kbps' + (attributes.vbr ? ' (vbr)' : '');
	if (attributes.sample_rate) return attributes.sample_rate / 1000 + ' kHz';
	return '';
}

export function baseName(virtualPath) {
	const index = virtualPath.lastIndexOf('\\');
	return index === -1 ? virtualPath : virtualPath.slice(index + 1);
}

export function folderName(virtualPath) {
	const index = virtualPath.lastIndexOf('\\');
	return index === -1 ? '' : virtualPath.slice(0, index);
}

export function formatDuration(seconds) {
	seconds = Math.round(seconds);
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	const ms = `${m}:${String(s).padStart(2, '0')}`;
	return h ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}` : ms;
}

export function formatTime(timestamp) {
	return new Date(timestamp * 1000).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
	});
}

export function formatCount(count) {
	return count.toLocaleString();
}

export function plural(count, noun, many = noun + 's') {
	return `${formatCount(count)} ${count === 1 ? noun : many}`;
}

export function fold(text) {
	return text
		.normalize('NFD')
		.replace(/\p{M}/gu, '')
		.toLowerCase();
}

export function effectiveBitrate({ bitrate, sample_rate, bit_depth }) {
	if (bitrate !== null) return bitrate;
	if (sample_rate && bit_depth) return Math.floor((sample_rate * bit_depth * 2) / 1000);
	return 0;
}

export function formatSpeed(bytesPerSecond) {
	return formatSize(bytesPerSecond) + '/s';
}

export function formatDay(timestamp) {
	return new Date(timestamp * 1000).toLocaleDateString([], {
		weekday: 'short', day: 'numeric', month: 'short', year: 'numeric',
	});
}

export function formatDateTime(timestamp) {
	return new Date(timestamp * 1000).toLocaleString([], {
		dateStyle: 'medium', timeStyle: 'short',
	});
}

const LINK = /((?:https?|slsk):\/\/[^\s<>"]+)/g;

export function segments(text) {
	return text.split(LINK).map((part, index) => ({ text: part, link: index % 2 === 1 }));
}
