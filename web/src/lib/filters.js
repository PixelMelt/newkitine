import { effectiveBitrate, fold } from './format.js';

const SIZE_UNITS = { b: 1, k: 1 << 10, m: 1 << 20, g: 1 << 30, t: 1024 ** 4 };

function parseSize(text) {
	const match = /^(\d+(?:\.\d+)?)([bkmgt]?)$/.exec(text);
	return match ? Number(match[1]) * SIZE_UNITS[match[2] || 'b'] : null;
}

function parseDuration(text) {
	if (!/^\d+(:\d{1,2}){0,2}$/.test(text)) return null;
	return text.split(':').map(Number).reduce((total, part) => total * 60 + part, 0);
}

function parseNumber(text) {
	return /^\d+$/.test(text) ? Number(text) : null;
}

function numericClauses(text, getValue, parseValue, bareIsMinimum) {
	const clauses = [];
	let invalid = false;
	for (const token of text.toLowerCase().split(/\s+/).filter(Boolean)) {
		const operator = /^[<>!=]/.exec(token)?.[0] ?? (bareIsMinimum ? '>' : '=');
		const wanted = parseValue(token.replace(/^[<>!=]/, ''));
		if (wanted === null) {
			invalid = true;
			continue;
		}
		if (operator === '>') clauses.push((row) => getValue(row) >= wanted);
		if (operator === '<') clauses.push((row) => getValue(row) <= wanted);
		if (operator === '=') clauses.push((row) => getValue(row) === wanted);
		if (operator === '!') clauses.push((row) => getValue(row) !== wanted);
	}
	return { clauses, invalid };
}

function phrases(text) {
	return text
		.split('|')
		.map((phrase) => fold(phrase.trim()))
		.filter(Boolean);
}

export const emptyFilters = () => ({
	include: '',
	exclude: '',
	type: '',
	size: '',
	bitrate: '',
	duration: '',
	freeSlot: false,
});

export const activeFilterCount = (f) =>
	Object.values(f).filter((value) => (typeof value === 'string' ? value.trim() : value)).length;

export function compileFilters(f) {
	const clauses = [];
	if (f.include.trim()) {
		const wanted = phrases(f.include);
		clauses.push(({ folded }) => wanted.some((p) => folded.includes(p)));
	}
	if (f.exclude.trim()) {
		const banned = phrases(f.exclude);
		clauses.push(({ folded }) => !banned.some((p) => folded.includes(p)));
	}
	if (f.type.trim()) {
		const tokens = f.type.toLowerCase().split(/\s+/).filter(Boolean);
		const allow = tokens.filter((t) => !t.startsWith('!'));
		const deny = tokens.filter((t) => t.startsWith('!')).map((t) => t.slice(1));
		clauses.push(({ file }) => {
			const ext = file.name.split('.').pop().toLowerCase();
			if (deny.includes(ext)) return false;
			return allow.length === 0 || allow.includes(ext);
		});
	}
	const size = numericClauses(f.size, ({ file }) => file.size, parseSize, true);
	const bitrate = numericClauses(
		f.bitrate, ({ file }) => effectiveBitrate(file.attributes), parseNumber, true,
	);
	const duration = numericClauses(
		f.duration, ({ file }) => file.attributes.length ?? 0, parseDuration, true,
	);
	clauses.push(...size.clauses, ...bitrate.clauses, ...duration.clauses);
	if (f.freeSlot) {
		clauses.push(({ response }) => response.free_upload_slots);
	}
	return {
		matches: (row) => clauses.every((clause) => clause(row)),
		invalid: { size: size.invalid, bitrate: bitrate.invalid, duration: duration.invalid },
	};
}
