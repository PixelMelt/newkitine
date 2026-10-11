import { writable } from 'svelte/store';

export function persisted(key, initial, isValid) {
	const stored = JSON.parse(localStorage.getItem(key));
	const store = writable(stored !== null && isValid(stored) ? stored : initial);
	store.subscribe((value) => localStorage.setItem(key, JSON.stringify(value)));
	return store;
}

export const isRecord = (value) =>
	typeof value === 'object' && value !== null && !Array.isArray(value);
