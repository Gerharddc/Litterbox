# Architecture

## Core Components

### lbx-init (Container Entrypoint)

A minimal, statically-linked binary designed to run inside Litterbox containers. It handles two commands:

- **entrypoint**: Initializes the user's home directory and launches the login shell inside the container. Handles privilege dropping and wait behaviour for background processes.
- **wait**: Blocks until the session lock file is empty (all terminals closed), then waits for child processes to exit. Also starts `xwayland-satellite` as a background daemon if it is installed.

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

- The container entrypoint is `/lbx-init wait`, which blocks until the session lock file is empty; this is detected using inotify for efficient watching
- If `xwayland-satellite` is present in `$PATH`, it is started as a background daemon; `DISPLAY` is passed as a positional argument if set

**Stopping:**

- When the daemon detects no active sessions, it exits
- This triggers the container's entrypoint to return
- Podman automatically stops the container

### File System Isolation

The isolation model restricts what the container can access:

**Mounted Paths:**

- `~/Litterbox/homes/{name}` → `/home/user` (user's home inside container)
- `~/Litterbox/mounts/{name}` → `/home/user/shared` (read-only, `rslave`; see below)
- Dynamically mounted projects → `/home/user/shared/{project}` (read-write)
- `/lbx-init` (binary, read-only) → Container entrypoint
- `/session.lock` (host's session lock, read-only) → Used for waiting
- `/tmp/ssh-agent.sock` → SSH agent socket for key access
- `/tmp/pipewire-0` (optional) → PipeWire socket for audio
- Wayland socket → Host's Wayland display

**Isolation Boundaries:**

- User namespace is set to `keep-id` to match host user
- Root filesystem is fully isolated (no access to host except mounted paths)
- Network mode defaults to `pasta` (user-mode networking stack)

### Dynamic Project Mounts

`litterbox enter NAME --mount [PATH]` makes a project directory available
without moving it into `~/Litterbox/homes`:

1. Each Litterbox has a dedicated host directory,
   `~/Litterbox/mounts/{name}`. It is bind-mounted into the container at
   `/home/user/shared` with `rslave` propagation and mounted read-only.
2. `litterbox` bind-mounts each requested project into that directory on the
   host (`~/Litterbox/mounts/{name}/{project}`). Because the container's mount
   is a slave of the host mount, the new mount appears in the running
   container's mount namespace.
3. The entrypoint receives an `--allow` argument for every visible mount. This
   is required because Landlock rules are attached to inodes: a bind-mounted
   project inode is not below the container home inode, so it would otherwise
   be denied. The shared directory itself is also allowed so that it can be
   listed. Sessions that started before a mount was added do not get a rule for
   it. This means a process that is already running in another session cannot
   read a project that is mounted later, even though the mount point is
   visible.

Podman requires the host source mount to be shared or slave for `rslave` to
work. If the directory's containing mount is not already shared, `litterbox`
creates a bind mount of the directory and marks it `rshared` (via `sudo`).
Host bind mounts cannot be created by an unprivileged process, so mounting and
unmounting projects always requires `sudo`; this matches the existing
`litterbox device` behaviour.

Mounts are scoped to a running container. While it runs, all of its sessions
share the same set of project mounts. When the container is stopped and
entered again, stale project mounts are unmounted before the new set is
applied. New containers record the state of the shared mount in the
`work.litterbox.shared` label (`ready` when propagation is configured,
`configured` otherwise). Containers without the label, or with `configured`,
are recreated (after committing their filesystem) the first time `--mount` is
used while they are stopped.

### Session Tracking

Each terminal session is tracked via PID:

1. When `litterbox enter` runs, it adds the terminal's PID to the session lockfile
2. When the terminal exits, the PID is removed from the lockfile
3. The daemon periodically checks and cleans up PIDs for dead processes
4. When the lockfile is empty, the container stops

This allows multiple concurrent sessions (e.g., multiple terminals) to share one container.

### Entry Flow

When a user runs `litterbox enter NAME`:

1. **Container Check** - Verify the container exists; if a previous re-creation removed it, recreate it from the existing image and saved settings.
2. **Mount Support Preparation** - When `--mount` is used, resolve the sources and verify that the host shared directory has shared propagation. If necessary, make it a shared mount with `sudo` and recreate a stopped container so that it can be created with `rslave`.
3. **Settings Reload** - Re-read `~/Litterbox/definitions/NAME.ron`. If it differs from the `work.litterbox.settings` fingerprint label recorded on the container and the container is stopped, commit the container's filesystem to its image and recreate the container with the new settings. Running containers are left untouched; their settings are applied on the next re-creation.
4. **Mount Lock and Session Registration** - Take a per-Litterbox mount lock, clean dead PIDs from the session lockfile, remove stale project mounts if the container is stopped, and add the terminal's PID to the session lockfile.
5. **Container and Daemon Start** - Start the daemon if necessary (it binds the SSH agent socket) and start the container if it is not running.
6. **Project Mounts** - Bind-mount the requested projects into the host shared directory. The container's slave mount receives them immediately.
7. **Entrypoint** - Run `/lbx-init entrypoint --allow ...` to add Landlock rules for the visible mounts, initialize the user's home directory (only on first entry), and enter the default shell.
8. **Shell Launch** - Start the user's shell inside the container, defaulting the working directory to the first requested mount.
