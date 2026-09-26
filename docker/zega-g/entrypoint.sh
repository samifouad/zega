#!/bin/sh
# The container's init process (Fly Machines' default) is root so a fresh
# Fly volume mounted at /data — created root:root 0755 by Fly, not by this
# image — can be handed to the non-root `zega` user before the server itself
# ever runs. See the Dockerfile's comment for the community report this
# guards against.
#
# `chown` is non-recursive: only /data's own ownership needs to change once;
# `zega` then owns every file it creates under it (including /data/zega, the
# data directory `zega start` is given), so this stays a fixed-cost startup
# check, not an O(data size) walk on every restart.
set -eu

if [ -d /data ]; then
  chown zega:zega /data
fi

# Same reasoning for the token file: Fly's `files` (raw_value) writes
# /etc/zega/token before the machine's own entrypoint runs, root-owned; hand
# it to `zega` (mode stays owner-only, never group/world-readable) so the
# non-root server can still read its own bearer token.
if [ -f /etc/zega/token ]; then
  chown zega:zega /etc/zega/token
  chmod 0600 /etc/zega/token
fi

exec setpriv --reuid=zega --regid=zega --clear-groups --no-new-privs \
  /usr/local/bin/zega "$@"
