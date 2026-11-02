#!/bin/sh
# safe-restart.sh: restart the OpenEtch engine's systemd service only while no job is burning.
#
# It asks the engine (GET /api/machine) whether a run holds the machine (`running`, `job.active`) and refuses to
# restart while one does, unless --force. Even with --force the engine itself lets a running job finish before it
# exits (SIGTERM is a graceful shutdown, up to --shutdown-grace-secs), as long as the unit's TimeoutStopSec is at least
# that long; see ARCHITECTURE.md "Deployment".
#
# Usage: tools/pi/safe-restart.sh [--force] [--url http://127.0.0.1:7878] [--service openetch]
#
# Environment:
#   OPENETCH_TOKEN  the engine's access token, when it listens off loopback (sent as "Authorization: Bearer ...").
#                   The script never prints it and never puts it on a command line (it reaches curl on stdin).
#   OPENETCH_URL    the engine's address (default http://127.0.0.1:7878; --url wins).
#
# Exit status: 0 restarted, 1 error, 2 usage, 3 refused (a job is running, or the state could not be read).
# `systemctl restart` returns once the engine has stopped, which can take as long as the job it lets finish.
#
# Needs curl, and jq or python3 to read the JSON. POSIX sh. Licence: MIT OR Apache-2.0 (part of OpenEtch).

set -u
# Never trace: `sh -x safe-restart.sh` would print the printf line below with the token expanded.
set +x

url="${OPENETCH_URL:-http://127.0.0.1:7878}"
service="openetch"
force=0

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --force) force=1 ;;
        --url) [ $# -ge 2 ] || { echo "safe-restart: --url needs a value" >&2; exit 2; }; url="$2"; shift ;;
        --url=*) url="${1#--url=}" ;;
        --service) [ $# -ge 2 ] || { echo "safe-restart: --service needs a value" >&2; exit 2; }; service="$2"; shift ;;
        --service=*) service="${1#--service=}" ;;
        -h|--help) usage; exit 0 ;;
        *) echo "safe-restart: unknown option '$1' (see --help)" >&2; exit 2 ;;
    esac
    shift
done

case "$service" in
    *[!A-Za-z0-9@._-]*|'') echo "safe-restart: bad service name" >&2; exit 2 ;;
esac

if ! command -v curl >/dev/null 2>&1; then
    echo "safe-restart: curl is needed" >&2
    exit 1
fi

# GET /api/machine. The header comes from stdin (curl -H @-), so the token is never an argument (ps shows those).
# printf is a shell builtin: no process sees the token on its command line either.
fetch_status() {
    if [ -n "${OPENETCH_TOKEN:-}" ]; then
        printf 'Authorization: Bearer %s\n' "$OPENETCH_TOKEN" |
            curl -fsS --max-time 10 -H @- "$url/api/machine"
    else
        curl -fsS --max-time 10 "$url/api/machine"
    fi
}

# Prints "running active shutting_down run_id state remaining_s" from the status JSON on stdin.
parse_status() {
    if command -v jq >/dev/null 2>&1; then
        jq -r '[(.running == true), (.job.active == true), (.shutting_down == true),
                (.job.run_id // "-"), (.job.state // "-"), ((.job.remaining_s // "-") | tostring)] | map(tostring) | join(" ")'
    elif command -v python3 >/dev/null 2>&1; then
        python3 -I -c '
import json, sys
m = json.load(sys.stdin)
j = m.get("job") or {}
def b(v): return "true" if v is True else "false"
r = j.get("remaining_s")
print(b(m.get("running")), b(j.get("active")), b(m.get("shutting_down")), j.get("run_id") or "-", j.get("state") or "-",
      "-" if r is None else r)'
    else
        return 1
    fi
}

if ! status="$(fetch_status)"; then
    echo "safe-restart: cannot read $url/api/machine (engine down, or OPENETCH_TOKEN missing or wrong)." >&2
    if [ "$force" -eq 0 ]; then
        echo "safe-restart: refusing to restart without knowing whether a job runs; use --force to restart anyway." >&2
        exit 3
    fi
    echo "safe-restart: --force: restarting anyway." >&2
else
    if ! fields="$(printf '%s' "$status" | parse_status)"; then
        echo "safe-restart: cannot read the engine's answer (install jq or python3)." >&2
        [ "$force" -eq 1 ] || exit 3
        fields="unknown unknown unknown - - -"
    fi
    # shellcheck disable=SC2086 # split on purpose: six words (no globbing)
    set -f
    set -- $fields
    set +f
    running="$1"; active="$2"; shutting="$3"; run_id="$4"; state="$5"; remaining="$6"
    if [ "$shutting" = "true" ]; then
        echo "safe-restart: the engine is already shutting down (it waits for the running job, then exits)."
    fi
    if [ "$running" = "true" ] || [ "$active" = "true" ]; then
        echo "safe-restart: a job holds the machine: run $run_id, state $state, about $remaining s left." >&2
        if [ "$force" -eq 0 ]; then
            echo "safe-restart: refusing to restart while a job runs. Let it finish (or press Stop), or use --force:" >&2
            echo "safe-restart: the engine then still waits for the job up to its grace limit before it exits." >&2
            exit 3
        fi
        echo "safe-restart: --force: restarting; the engine finishes the job first (up to its grace limit)." >&2
    elif [ "$running" = "unknown" ]; then
        echo "safe-restart: --force: restarting without knowing whether a job runs." >&2
    fi
fi

if [ "$(id -u)" -eq 0 ]; then
    systemctl restart "$service"
else
    sudo systemctl restart "$service"
fi
rc=$?
if [ "$rc" -ne 0 ]; then
    echo "safe-restart: systemctl restart $service failed ($rc)." >&2
    exit 1
fi
echo "safe-restart: $service restarted."
