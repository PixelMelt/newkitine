import { post } from './api.js';
import { report } from './state/session.js';

const connect = { label: 'Connect', run: () => act(() => post('/connect'), { failed: 'Not connected' }) };

function explain(error, on) {
	if (error.status === undefined) return { cause: "can't reach the Newkitine server" };
	if (on[error.status]) return { cause: on[error.status] };
	if (error.status === 409) return { cause: 'not logged in to Soulseek', action: connect };
	if (error.status === 400 || error.status === 422) return { cause: error.message };
	console.error(error);
	return { cause: `server error ${error.status}, see the server log` };
}

export async function act(request, { failed, done, action, on = {}, where, then }) {
	let result;
	try {
		result = await request();
	} catch (error) {
		const { cause, action: remedy } = explain(error, on);
		const text = `${failed}: ${cause}.`;
		if (where) where(text, error);
		else report.error(text, remedy);
		return false;
	}
	if (then) then(result);
	if (done) report.done(typeof done === 'function' ? done(result) : done, action);
	return true;
}

export function copy(text, what) {
	const field = document.createElement('textarea');
	const focused = document.activeElement;
	field.value = text;
	field.style.position = 'fixed';
	field.style.opacity = '0';
	document.body.append(field);
	field.select();
	const copied = document.execCommand('copy');
	field.remove();
	focused.focus();
	if (copied) report.done(`Copied ${what}`);
	else report.error(`Could not copy ${what}: the browser refused clipboard access.`);
}
