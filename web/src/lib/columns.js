import { formatCount, formatDuration, formatQuality, effectiveBitrate } from './format.js';

export const numeric = (key, label, width, value, format = formatCount) => ({
	key, label, width, align: 'end', desc: true, text: (r) => format(value(r)), sortValue: value,
});

export const quality = (attributes) => ({
	key: 'quality', label: 'Quality', width: 124, desc: true,
	text: (r) => formatQuality(attributes(r)), sortValue: (r) => effectiveBitrate(attributes(r)),
});

export const length = (attributes) =>
	numeric('length', 'Length', 64, (r) => attributes(r).length ?? 0, (n) => (n ? formatDuration(n) : ''));
