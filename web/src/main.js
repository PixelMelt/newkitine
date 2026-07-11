import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';

document.documentElement.dataset.theme = localStorage.getItem('theme') ?? 'dark';

export default mount(App, { target: document.getElementById('app') });
