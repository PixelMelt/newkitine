function clipped(host) {
	return [host, ...host.querySelectorAll('*')].some(
		(element) =>
			element.scrollWidth > element.clientWidth &&
			getComputedStyle(element).textOverflow === 'ellipsis',
	);
}

export function showHovertext(event) {
	const host = event.target.closest('[data-hovertext]');
	if (!host) return;
	const lines = [];
	if (clipped(host)) lines.push(host.textContent.trim());
	if (host.dataset.hovertext) lines.push(host.dataset.hovertext);
	if (lines.length) host.title = lines.join('\n');
	else host.removeAttribute('title');
}
