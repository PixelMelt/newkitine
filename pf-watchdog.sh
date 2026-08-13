#!/bin/sh
# Recover gluetun-newkitine's Proton NAT-PMP lease when it dies.
#
# When the lease drops, gluetun retries the mapping request against the same
# gateway forever but never reconnects the tunnel, so it never self-heals. The
# VPN itself stays up the whole time, so a connectivity healthcheck never fires.
#
# Reconnecting the tunnel forces a fresh lease on a different node. newkitine
# then picks the new port up on its own: src/app/gluetun.rs polls
# GET /v1/portforward and hot-applies it, so no VPN_PORT_FORWARDING_UP_COMMAND
# is needed here.
#
# A soft reconnect through the control server is used instead of restarting the
# gluetun container, because newkitine shares its network namespace and a
# container restart takes newkitine down with it.
set -u

PORT_FILE="${PORT_FILE:-/gluetun/forwarded_port}"
CTRL="${CTRL:-http://127.0.0.1:8000}"
INTERVAL="${INTERVAL:-60}"    # seconds between checks
GRACE="${GRACE:-5}"           # consecutive dead checks before reconnecting
COOLDOWN="${COOLDOWN:-900}"   # seconds to settle after a reconnect

log() { echo "$(date '+%Y-%m-%dT%H:%M:%S') $*"; }

log "watching $PORT_FILE every ${INTERVAL}s (grace ${GRACE} checks)"

dead=0
while :; do
	# Guard rather than redirect stderr: the shell reports a failed input
	# redirect before 2>/dev/null would apply, which leaks into the log.
	port=""
	[ -r "$PORT_FILE" ] && port="$(tr -dc '0-9' < "$PORT_FILE")"
	vpn="$(curl -fsS --max-time 10 "$CTRL/v1/vpn/status" 2>/dev/null |
		sed -n 's/.*"status":[[:space:]]*"\([a-z]*\)".*/\1/p')"

	if [ -n "$port" ] && [ "$port" != "0" ]; then
		[ "$dead" -gt 0 ] && log "lease healthy again on port $port"
		dead=0
	elif [ "$vpn" != "running" ]; then
		# Mid-reconnect, or stopped on purpose. Not ours to fix.
		log "vpn status '${vpn:-unreachable}' is not running; waiting"
		dead=0
	else
		dead=$((dead + 1))
		log "no forwarded port in $PORT_FILE while vpn is running ($dead/$GRACE)"

		if [ "$dead" -ge "$GRACE" ]; then
			log "reconnecting tunnel to force a fresh NAT-PMP lease"
			if curl -fsS --max-time 30 -X PUT -d '{"status":"stopped"}' \
				"$CTRL/v1/vpn/status" >/dev/null 2>&1; then
				log "tunnel stopped"
			else
				log "stop request failed"
			fi
			sleep 5
			if curl -fsS --max-time 30 -X PUT -d '{"status":"running"}' \
				"$CTRL/v1/vpn/status" >/dev/null 2>&1; then
				log "tunnel started"
			else
				log "start request failed"
			fi
			dead=0
			log "cooling down ${COOLDOWN}s before checking again"
			sleep "$COOLDOWN"
		fi
	fi

	sleep "$INTERVAL"
done
