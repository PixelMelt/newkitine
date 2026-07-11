import { writable } from 'svelte/store';

export const menu = writable(null);

export function openMenu(event, items) {
  event.preventDefault();
  event.stopPropagation();
  menu.set({ x: event.clientX, y: event.clientY, items });
}

export function closeMenu() {
  menu.set(null);
}
