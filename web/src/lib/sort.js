const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

export const compareNames = (a, b) => collator.compare(a, b);

function compare(a, b) {
	if (typeof a === 'number' && typeof b === 'number') return a - b;
	return collator.compare(String(a), String(b));
}

export function sortRows(rows, sort, columns) {
	if (!sort) return rows;
	const column = columns.find((c) => c.key === sort.key);
	const value = column.sortValue ?? column.text;
	return rows
		.map((row, index) => ({ row, index, value: value(row) }))
		.sort((a, b) => compare(a.value, b.value) * sort.dir || a.index - b.index)
		.map((entry) => entry.row);
}
