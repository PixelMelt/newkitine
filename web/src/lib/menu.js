import { writable } from 'svelte/store';

import { copy } from './act.js';

export const menu = writable(null);

export function openMenuAt(point, items) {
	menu.set({ x: point.x, y: point.y, items, returnFocus: document.activeElement });
}

export function openMenu(event, items) {
	event.preventDefault();
	event.stopPropagation();
	openMenuAt({ x: event.clientX, y: event.clientY }, items);
}

export function closeMenu() {
	menu.set(null);
}

export const copyPaths = (paths) => ({
	label: paths.length > 1 ? `Copy ${paths.length} File Paths` : 'Copy File Path',
	action: () => copy(paths.join('\n'), paths.length > 1 ? 'file paths' : 'file path'),
});
