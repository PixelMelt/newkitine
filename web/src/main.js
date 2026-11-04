import { mount } from 'svelte';
import App from './App.svelte';
import { report } from './lib/stores.js';
import { showHovertext } from './lib/hovertext.js';
import './app.css';

window.addEventListener('unhandledrejection', (event) => {
	report.error(`Unexpected error: ${event.reason?.message ?? event.reason}`);
});

document.addEventListener('mouseover', showHovertext);

export default mount(App, { target: document.getElementById('app') });
