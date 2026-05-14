# Architecture

## Core Components

### lbx-init (Container Entrypoint)

A minimal, statically-linked binary designed to run inside Litterbox containers. It handles two commands:

- **entrypoint**: Initializes the user's home directory and launches the login shell inside the container. Handles privilege dropping and wait behaviour for background processes.
- **wait**: Blocks until the session lock file is empty (all sessions closed), then waits for child processes to exit.

**Key characteristics:**

- Independent of the main `litterbox` binary
- Statically linked and always built for Linux
- Supports cross-compilation (useful on non-Linux hosts)
- Mounted at `/lbx-init` in the container

### The Daemon

A long-running daemon process manages each Litterbox. It is started automatically when entering a Litterbox and persists until the container is stopped.

**Responsibilities:**

- Manages the SSH agent server for key access
- Monitors active sessions via lockfiles
- Automatically stops the container when all sessions close

**Lockfiles** in `~/Litterbox/`:

- `.daemon-{LBX_NAME}.lock` - Contains the daemon's PID. Prevents multiple daemons from running simultaneously.
- `.session-{LBX_NAME}.lock` - Contains PIDs of all active terminal sessions. The daemon periodically cleans stale PIDs and stops the container when this file becomes empty.

### SSH Key Architecture

Litterbox implements a custom SSH agent to control key access.

**Components:**

1. **Key Storage** - Keys are stored in `~/Litterbox/keys.ron`, encrypted with a user-provided password.

2. **SSH Agent Server** - A custom agent implementation built on russh that:
   - Runs inside the daemon as an async task
   - Listens on a Unix socket mounted into the container
   - Decrypts keys only when needed and registers them with the agent

3. **Confirmation System** - Before any key operation (sign, list, add, remove), the agent spawns a GUI confirmation dialog using. Users can:
   - Approve the single request
   - Approve for the entire session (subsequent requests of the same type are auto-approved)
   - Decline the request

A new short-livived process is spawned for each confirmation dialog. This is mainly to work around threading issues with the GUI system.

### Container Lifecycle

**Building:**

- Litterbox uses Dockerfile templates to build images
- Images are tagged with `work.litterbox.name` label for discovery
- Each Litterbox has both an image and a container

**Starting:**

- The container entrypoint is `/lbx-init wait`, which blocks until the session lock file is empty
- This is detected using inotify for efficient watching

**Stopping:**

- When the daemon detects no active sessions, it exits
- This triggers the container's `/lbx-init wait` to return
- Podman automatically stops the container

### File System Isolation

The isolation model restricts what the container can access:

**Mounted Paths:**

- `~/Litterbox/homes/{name}` → `/home/user` (user's home inside container)
- `/lbx-init` (binary, read-only) → Container entrypoint
- `/session.lock` (host's session lock, read-only) → Used for waiting
- `/tmp/ssh-agent.sock` → SSH agent socket for key access
- `/tmp/pipewire-0` (optional) → PipeWire socket for audio
- Wayland socket → Host's Wayland display

**Isolation Boundaries:**

- User namespace is set to `keep-id` to match host user
- Root filesystem is fully isolated (no access to host except mounted paths)
- Network mode defaults to `pasta` (user-mode networking stack)

### Session Tracking

Each terminal session is tracked via PID:

1. When `litterbox enter` runs, it adds the terminal's PID to the session lockfile
2. When the terminal exits, the PID is removed from the lockfile
3. The daemon periodically checks and cleans up PIDs for dead processes
4. When the lockfile is empty, the container stops

This allows multiple concurrent sessions (e.g., multiple terminals) to share one container.

### Entry Flow

When a user runs `litterbox enter NAME`:

1. **Container Check** - Verify container exists; start if not running
2. **Daemon Start** - If daemon not running, spawn it with the key password via stdin
3. **Session Registration** - Add terminal PID to session lockfile
4. **Entrypoint** - Run `/lbx-init entrypoint` to initialize user's home directory (only on first entry) and then enter the default shell
5. **Shell Launch** - Start user's shell inside the container
