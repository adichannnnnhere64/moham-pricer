#!/usr/bin/env bash
set -euo pipefail

rustc_bin="$1"
shift

filtered_args=()
skip_next=0

for arg in "$@"; do
  if [[ "$skip_next" == "1" ]]; then
    skip_next=0
    continue
  fi

  case "$arg" in
    --check-cfg)
      skip_next=1
      ;;
    --check-cfg=*)
      ;;
    --json=*)
      filtered_args+=("${arg/,future-incompat/}")
      ;;
    *)
      filtered_args+=("$arg")
      ;;
  esac
done

exec "$rustc_bin" "${filtered_args[@]}"
