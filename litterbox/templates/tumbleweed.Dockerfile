# syntax=docker/dockerfile:1.4
FROM registry.opensuse.org/opensuse/tumbleweed:latest

# Setup base system (we install weston to easily get all the Wayland deps)
RUN zypper refresh && \
    zypper in -y weston libvulkan_radeon libvulkan_intel openssh git iputils vulkan-tools curl iproute2 rsync awk

# Install the fish shell for a nicer experience (ADAPT TO YOUR OWN NEEDS)
RUN zypper in -y fish

# Setup support for X11 apps through xwayland (ADAPT TO YOUR OWN NEEDS)
RUN zypper in -y xwayland-satellite
ENV DISPLAY=:0

# Install development toolchain (ADAPT TO YOUR OWN NEEDS)
RUN zypper in -y gcc

# Setup non-root user for added security
# (NB Litterbox assumes you do this step)
ARG USER
ARG UID
ARG GID
RUN groupadd -g $GID $USER || true
RUN useradd -m $USER -u $UID -g $GID
WORKDIR /home/$USER

# We do not install things directly into $HOME here as they will get nuked
# once the home directory gets mounted. Instead we use a script that runs
# at start-up to construct the home directory the first time.
#
# A benefit of not installing things directly into home means that they do
# need to be re-installed when the container gets rebuilt.
RUN <<'EOF'
# Create the script using a nested heredoc
cat <<'EOT' > /prep-home.sh
#!/usr/bin/env sh

# -------------------------------------
# ADAPT THIS EXAMPLE TO YOUR OWN NEEDS
# -------------------------------------
# curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
EOT

chmod +x /prep-home.sh
chown $USER /prep-home.sh
EOF

# Set LANG to enable UTF-8 support
ENV LANG=en_US.UTF-8

# Enter the fish shell by default (ADAPT TO YOUR OWN NEEDS)
ENV SHELL=fish
RUN chsh -s /usr/bin/fish $USER
