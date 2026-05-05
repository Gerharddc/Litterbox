#!/usr/bin/env sh
set -eu

# Tiny in-container helper used on non-Linux hosts.
#
# Subcommands:
# - wait: blocks until /session.lock is removed or empty.
# - entrypoint: drops privileges to `user` unless --root is set, then runs
#   either a login shell or the provided command.

subcmd="${1:-}"
if [ -z "$subcmd" ]; then
  echo "missing subcommand" >&2
  exit 2
fi
shift

case "$subcmd" in
  wait)
    # Poll the lock file used by the host daemon/session manager.
    # We exit once the file disappears or becomes empty.
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
    # Parse a strict subset of the Rust entrypoint flags.
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

    # Best-effort ownership fix so runtime sockets under XDG_RUNTIME_DIR stay
    # usable after dropping privileges.
    if [ -n "${XDG_RUNTIME_DIR:-}" ] && [ -n "$uid" ] && [ -n "$gid" ]; then
      chown "$uid:$gid" "$XDG_RUNTIME_DIR" >/dev/null 2>&1 || true
    fi

    # Resolve SHELL to an absolute path. If SHELL is missing or unresolvable,
    # fall back to /bin/sh.
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

    # Command mode: execute argv directly as root, or preserve argv exactly
    # when invoking through `su` (no command-string concatenation).
    if [ "$#" -gt 0 ]; then
      if [ "$root" = true ]; then
        exec "$@"
      else
        exec su -m user -s /bin/sh -c 'exec "$@"' sh "$@"
      fi
    else
      # Interactive mode: start a login shell as root or as `user`.
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
