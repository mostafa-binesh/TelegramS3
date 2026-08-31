#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
  set -- server
fi

if [ "$1" = "server" ]; then
  /usr/local/bin/telegram-s3 config check
fi

exec /usr/local/bin/telegram-s3 "$@"
