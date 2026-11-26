#!/bin/sh
# Prints "yes" when one of the changed paths reaches the dashboard's checks, "no" otherwise.
# The paths come as arguments, or one per line on standard input when there is none; standard
# input is read to its end, so the command writing it never gets a broken pipe.
# The Gate workflow's dashboard job runs its checks on a pull request only on "yes".
set -eu

found=no

check() {
  case "$1" in
    dashboard/* | crates/llm-gateway/src/dashboard.html | .github/workflows/gate.yml) found=yes ;;
  esac
}

if [ "$#" -gt 0 ]; then
  for path in "$@"; do
    check "$path"
  done
else
  while IFS= read -r path || [ -n "$path" ]; do
    check "$path"
  done
fi
echo "$found"
