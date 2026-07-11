const SIZE_UNITS = { b: 1, k: 1 << 10, m: 1 << 20, g: 1 << 30, t: 1024 ** 4 };

function parseSize(text) {
	const match = /^([\d.]+)\s*([bkmgt]?)/.exec(text);
	if (!match) return null;
	return Number(match[1]) * SIZE_UNITS[match[2] || 'b'];
}

function parseDuration(text) {
	const parts = text.split(':').map(Number);
	if (parts.some(Number.isNaN)) return null;
	return parts.reduce((total, part) => total * 60 + part, 0);
}

function numericClauses(text, getValue, parseValue, bareIsMinimum) {
	const clauses = [];
	for (const token of text.toLowerCase().split(/\s+/).filter(Boolean)) {
		const operator = /^[<>!=]/.exec(token)?.[0] ?? (bareIsMinimum ? '>' : '=');
		const wanted = parseValue(token.replace(/^[<>!=]/, ''));
		if (wanted === null) continue;
		if (operator === '>') clauses.push((row) => getValue(row) >= wanted);
		if (operator === '<') clauses.push((row) => getValue(row) <= wanted);
		if (operator === '=') clauses.push((row) => getValue(row) === wanted);
		if (operator === '!') clauses.push((row) => getValue(row) !== wanted);
	}
	return clauses;
}

function phrases(text) {
	return text
		.toLowerCase()
		.split('|')
		.map((phrase) => phrase.trim())
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

export function compileFilters(f) {
	const clauses = [];
	if (f.include.trim()) {
		const wanted = phrases(f.include);
		clauses.push(({ file }) => wanted.some((p) => file.name.toLowerCase().includes(p)));
	}
	if (f.exclude.trim()) {
		const banned = phrases(f.exclude);
		clauses.push(({ file }) => !banned.some((p) => file.name.toLowerCase().includes(p)));
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
	clauses.push(...numericClauses(f.size, ({ file }) => file.size, parseSize, true));
	clauses.push(
		...numericClauses(f.bitrate, ({ file }) => file.attributes?.bitrate ?? 0, Number, false),
	);
	clauses.push(
		...numericClauses(
			f.duration,
			({ file }) => file.attributes?.length ?? 0,
			parseDuration,
			false,
		),
	);
	if (f.freeSlot) {
		clauses.push(({ response }) => response.free_upload_slots);
	}
	return (row) => clauses.every((clause) => clause(row));
}
