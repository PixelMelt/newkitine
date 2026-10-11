import { writable } from 'svelte/store';

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
