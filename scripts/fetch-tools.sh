#!/bin/sh
# Python is needed only on the build machine, never on the user's machine.
set -eu
exec python3 "$(dirname "$0")/fetch-tools.py" "$@"
