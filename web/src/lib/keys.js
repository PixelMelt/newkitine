const mac = /Mac|iPhone|iPad/.test(navigator.platform);

export const hasCommand = (event) => (mac ? event.metaKey : event.ctrlKey);

export function isTyping(event) {
	const target = event.target;
	return (
		event.isComposing ||
		target.isContentEditable ||
		target.tagName === 'TEXTAREA' ||
		target.tagName === 'SELECT' ||
		(target.tagName === 'INPUT' && !['checkbox', 'radio', 'button'].includes(target.type))
	);
}

export function onEnter(run) {
	return (event) => {
		if (event.key !== 'Enter' || event.isComposing || event.keyCode === 229) return;
		event.preventDefault();
		run(event);
	};
}

export const startsTypeahead = (event) =>
	event.key.length === 1 && !/[\s\d,.<>/?]/.test(event.key);
