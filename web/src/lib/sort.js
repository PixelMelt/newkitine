export function sortRows(rows, sort, accessors = {}) {
	if (!sort.key) return rows;
	const value = accessors[sort.key] ?? ((row) => row[sort.key]);
	return [...rows].sort((a, b) => compare(value(a), value(b)) * sort.dir);
}

function compare(a, b) {
	if (typeof a === 'number' && typeof b === 'number') return a - b;
	return String(a ?? '').localeCompare(String(b ?? ''), undefined, {
		numeric: true,
		sensitivity: 'base',
	});
}
