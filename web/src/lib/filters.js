import { effectiveBitrate, fold } from './format.js';

const SPLIT_DIGITS = /(?:[|&\s])+(?<![<>!=]\s)/;
const SPLIT_TEXT = /(?:[|&,;\s])+(?<!!\s)/;
const APPROXIMATE_BELOW = 104857600;

const OPERATIONS = {
	'<': (a, b) => a < b,
	'<=': (a, b) => a <= b,
	'==': (a, b) => a === b,
	'!=': (a, b) => a !== b,
	'>=': (a, b) => a >= b,
	'>': (a, b) => a > b,
};

function splitOperator(condition) {
	const two = condition.slice(0, 2);
	if (two in OPERATIONS) return [two, condition.slice(2)];
	if (condition[0] === '<' || condition[0] === '>') return [condition[0], condition.slice(1)];
	if (condition[0] === '=' || condition[0] === '!') return [`${condition[0]}=`, condition.slice(1)];
	return ['>=', condition];
}

function parseSize(text) {
	let rest = text;
	let base = 1024;
	if (rest.endsWith('b')) {
		base = 1000;
		rest = rest.slice(0, -1);
	}
	if (rest.endsWith('i')) {
		base = 1024;
		rest = rest.slice(0, -1);
	}
	const power = { k: 1, m: 2, g: 3 }[rest.at(-1)] ?? 0;
	if (power) rest = rest.slice(0, -1);
	if (!/^\d+(\.\d+)?$/.test(rest)) return null;
	const factor = base ** power;
	const digit = Math.trunc(Number(rest) * factor);
	return { digit, adjust: factor > 1024 && digit < APPROXIMATE_BELOW ? factor / 8 : factor };
}

function parseCount(text) {
	if (/^\d+$/.test(text)) return { digit: Number(text), adjust: 0 };
	if (!/^\d+(:\d+){1,2}$/.test(text)) return null;
	const digit = text.split(':').map(Number).reduce((total, part) => total * 60 + part, 0);
	return { digit, adjust: 0 };
}

function numericClause(text, getValue, parseValue) {
	const conditions = [];
	let invalid = false;
	for (const token of text.toLowerCase().trim().split(SPLIT_DIGITS).filter(Boolean)) {
		const [operator, digit] = splitOperator(token);
		const parsed = parseValue(digit.trim());
		if (parsed) conditions.push({ operator, ...parsed });
		else invalid = true;
	}
	const clause = (row) => {
		const value = getValue(row);
		let allowed = false;
		let blocked = false;
		for (const { operator, digit, adjust } of conditions) {
			if (digit - adjust <= value && value <= digit + adjust) {
				if (operator === '==') return true;
				if (operator === '!=') return false;
			}
			if (value && OPERATIONS[operator](value, digit) && !blocked) allowed = true;
			else blocked = true;
		}
		return allowed && !blocked;
	};
	return { clauses: conditions.length ? [clause] : [], invalid };
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
		const tokens = f.type.toLowerCase().trim().split(SPLIT_TEXT).filter(Boolean);
		const suffix = (ext) => (ext.startsWith('.') ? ext : `.${ext}`);
		const allow = tokens.filter((t) => !t.startsWith('!')).map(suffix);
		const deny = tokens.filter((t) => t.startsWith('!')).map((t) => suffix(t.slice(1)));
		clauses.push(({ file }) => {
			const name = file.name.toLowerCase();
			if (deny.some((ext) => name.endsWith(ext))) return false;
			return allow.length === 0 || allow.some((ext) => name.endsWith(ext));
		});
	}
	const size = numericClause(f.size, ({ file }) => file.size, parseSize);
	const bitrate = numericClause(
		f.bitrate, ({ file }) => effectiveBitrate(file.attributes), parseCount,
	);
	const duration = numericClause(
		f.duration, ({ file }) => file.attributes.length ?? 0, parseCount,
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
