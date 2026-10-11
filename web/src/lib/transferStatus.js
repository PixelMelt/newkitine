const waiting = (text, hint) => ({ text, tone: 'waiting', hint });
const failed = (text, hint = '') => ({ text, tone: 'failed', hint });
const stopped = (text, hint = '') => ({ text, tone: 'stopped', hint });

const EVERY_3_MIN = 'Retried every 3 minutes';
const EVERY_15_MIN = 'Retried every 15 minutes';

function downloadFailure(reason, user) {
	if (reason === 'user is offline') {
		return waiting(`Waiting for ${user} to come online`, 'Resumes by itself');
	}
	if (reason === 'connection timeout') return waiting(`Can't connect to ${user}, retrying`, EVERY_3_MIN);
	if (reason === 'connection closed') return waiting('Connection dropped, retrying', EVERY_3_MIN);
	if (reason === 'request timed out') return waiting(`No answer from ${user}, retrying`, EVERY_3_MIN);
	if (reason === 'upload failed') return waiting(`${user}'s side failed, retrying`, EVERY_3_MIN);
	if (reason === 'Pending shutdown.') return waiting(`${user} is shutting down, retrying`, EVERY_3_MIN);
	if (reason === 'File read error.') {
		return waiting(`${user} couldn't read the file, retrying`, EVERY_15_MIN);
	}
	if (reason.startsWith('local file error: ')) {
		return failed(
			`Can't write the file here: ${reason.slice(18)}`,
			`${EVERY_15_MIN}. Check the download folder and free space.`,
		);
	}
	if (reason.startsWith('cannot place finished download: ')) {
		return failed(
			`Downloaded, but can't move it into place: ${reason.slice(32)}`,
			`${EVERY_15_MIN}. Check the download folder and free space.`,
		);
	}
	if (reason === 'File not shared.') {
		return failed(`Not shared by ${user} any more`, 'Retry will ask again');
	}
	if (reason === 'Cancelled') return failed(`Cancelled by ${user}`);
	if (reason === '') return failed('Failed', 'No reason was recorded');
	return failed(`Refused by ${user}: "${reason}"`);
}

function uploadFailure(reason, user) {
	if (reason === 'Cancelled') return stopped(`Cancelled by ${user}`);
	if (reason === 'connection timeout') return failed(`Couldn't connect to ${user}`);
	if (reason === 'user is offline') return stopped(`${user} went offline`);
	if (reason === 'connection closed') return failed('Connection dropped');
	if (reason === 'server disconnected') return stopped('Stopped: lost the Soulseek connection');
	if (reason === 'interrupted by restart') return stopped('Stopped: Newkitine restarted');
	if (reason === 'File not shared.') return stopped('Denied: no longer shared');
	if (reason.startsWith('local file error: ')) return failed(`Can't read the file: ${reason.slice(18)}`);
	if (reason.startsWith('file changed size')) {
		return failed('File changed on disk during upload, rescan shares');
	}
	if (reason === '') return failed('Failed', 'No reason was recorded');
	return stopped(`Denied or failed: "${reason}"`);
}

export function describe(transfer, direction) {
	const user = transfer.username;
	const isDownload = direction === 'download';
	if (transfer.status === 'queued') {
		return transfer.queue_place
			? waiting(`Queued, place ${transfer.queue_place.toLocaleString()}`, `In ${user}'s upload queue`)
			: waiting('Queued', `Waiting for ${user}`);
	}
	if (transfer.status === 'transferring') {
		if (isDownload && transfer.size && transfer.bytes_done === transfer.size) {
			return { text: 'Finishing', tone: 'active', hint: 'Moving the file into the download folder' };
		}
		return { text: isDownload ? 'Downloading' : 'Uploading', tone: 'active', hint: '' };
	}
	if (transfer.status === 'finished') {
		return { text: 'Finished', tone: 'done', hint: transfer.file_path ?? '' };
	}
	if (transfer.status === 'aborted') {
		return stopped('Stopped', isDownload ? 'Stopped by you. Retry to queue it again.' : 'Stopped by you');
	}
	const reason = transfer.failure_reason ?? '';
	return isDownload ? downloadFailure(reason, user) : uploadFailure(reason, user);
}

export function shortState(transfer) {
	if (transfer.status === 'finished') return 'Done';
	if (transfer.status === 'aborted') return 'Stopped';
	if (transfer.status === 'transferring') {
		return transfer.size ? `${Math.floor((transfer.bytes_done / transfer.size) * 100)}%` : '0%';
	}
	const tone = describe(transfer, 'download').tone;
	return tone === 'failed' ? 'Failed' : tone === 'waiting' && transfer.status === 'failed' ? 'Waiting' : 'Queued';
}

const SELF_RETRIED = [
	'user is offline', 'connection timeout', 'connection closed', 'request timed out',
	'upload failed', 'Pending shutdown.', 'File read error.',
];

export const retriesByItself = (transfer) =>
	transfer.status === 'failed' &&
	(SELF_RETRIED.includes(transfer.failure_reason) ||
		(transfer.failure_reason ?? '').startsWith('local file error') ||
		(transfer.failure_reason ?? '').startsWith('cannot place finished download'));

export const isActive = (transfer) =>
	transfer.status === 'queued' || transfer.status === 'transferring';

const TONE_RANK = { failed: 0, active: 1, waiting: 2, stopped: 3, done: 4 };

export const toneRank = (tone) => TONE_RANK[tone];
