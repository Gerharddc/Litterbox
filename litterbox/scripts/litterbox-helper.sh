#!/usr/bin/env sh
set -eu

subcmd="${1:-}"
if [ -z "$subcmd" ]; then
  echo "missing subcommand" >&2
  exit 2
fi
shift

case "$subcmd" in
  wait)
    while :; do
      if [ ! -f /session.lock ]; then
        break
      fi

      if [ ! -s /session.lock ]; then
        break
      fi

      sleep 0.2
    done
    ;;

  entrypoint)
    uid=""
    gid=""
    root=false

    while [ "${1:-}" != "" ]; do
      case "$1" in
        --uid)
          [ "$#" -ge 2 ] || {
            echo "missing value for --uid" >&2
            exit 2
          }
          uid="$2"
          shift 2
          ;;
        --gid)
          [ "$#" -ge 2 ] || {
            echo "missing value for --gid" >&2
            exit 2
          }
          gid="$2"
          shift 2
          ;;
        --wait)
          [ "$#" -ge 2 ] || {
            echo "missing value for --wait" >&2
            exit 2
          }
          shift 2
          ;;
        --root) root=true; shift ;;
        --) shift; break ;;
        *)
          echo "unsupported entrypoint option: $1" >&2
          exit 2
          ;;
      esac
    done

    if [ -n "${XDG_RUNTIME_DIR:-}" ] && [ -n "$uid" ] && [ -n "$gid" ]; then
      chown "$uid:$gid" "$XDG_RUNTIME_DIR" >/dev/null 2>&1 || true
    fi

    shell="${SHELL:-/bin/sh}"
    shell_path="$shell"

    case "$shell" in
      /*) ;;
      *)
        resolved="$(command -v "$shell" 2>/dev/null || true)"
        if [ -n "$resolved" ]; then
          shell_path="$resolved"
        else
          shell_path="/bin/sh"
        fi
        ;;
    esac

    if [ "$#" -gt 0 ]; then
      if [ "$root" = true ]; then
        exec "$@"
      else
        exec su -m user -s /bin/sh -c 'exec "$@"' sh "$@"
      fi
    else
      if [ "$root" = true ]; then
        exec "$shell_path" -l
      else
        exec su -m user -s "$shell_path"
      fi
    fi
    ;;

  *)
    echo "unsupported helper subcommand: $subcmd" >&2
    exit 2
    ;;
esac
