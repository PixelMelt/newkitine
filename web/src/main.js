import { mount } from 'svelte';
import App from './App.svelte';
import { report } from './lib/stores.js';
import './app.css';

window.addEventListener('unhandledrejection', (event) => {
	report.error(`Unexpected error: ${event.reason?.message ?? event.reason}`);
});

export default mount(App, { target: document.getElementById('app') });
