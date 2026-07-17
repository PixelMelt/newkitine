async function request(method, path, body) {
	const options = { method };
	if (body !== undefined) {
		options.headers = { 'Content-Type': 'application/json' };
		options.body = JSON.stringify(body);
	}
	const response = await fetch('/api' + path, options);
	const text = await response.text();
	if (!response.ok) {
		let detail;
		try {
			detail = JSON.parse(text).error;
		} catch {
			detail = text;
		}
		const error = new Error(detail || `${method} ${path}: ${response.status}`);
		error.status = response.status;
		throw error;
	}
	return text ? JSON.parse(text) : null;
}

export const get = (path) => request('GET', path);
export const post = (path, body) => request('POST', path, body);
export const put = (path, body) => request('PUT', path, body);
export const del = (path) => request('DELETE', path);
