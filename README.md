<!-- exclude:start -->

# Litterbox

<p align="center">
  <img src="litterbox/assets/cat.svg" alt="Centered SVG" width="200"/>
</p>

[![Build and Test](https://github.com/Gerharddc/litterbox/actions/workflows/build-and-test.yml/badge.svg)](https://github.com/Gerharddc/litterbox/actions/workflows/build-and-test.yml) [![Tag on Version Change](https://github.com/Gerharddc/litterbox/actions/workflows/tag-on-version-change.yml/badge.svg)](https://github.com/Gerharddc/litterbox/actions/workflows/tag-on-version-change.yml) [![Make Release](https://github.com/Gerharddc/litterbox/actions/workflows/make-release.yml/badge.svg)](https://github.com/Gerharddc/litterbox/actions/workflows/publish-installer.yml) [![Publish Website](https://github.com/Gerharddc/litterbox/actions/workflows/publish-website.yml/badge.svg)](https://github.com/Gerharddc/litterbox/actions/workflows/publish-website.yml)

<!-- exclude:end -->

Litterbox is a Linux sandbox environment catered to the needs of developers. Its primary goal is to provide SOME isolation between a containerised development environment and a host system. Its secondary goal is to provide a repeatable and documented environment for development.

The isolation goal is to prevent rogue processes (which might arrive through a supply chain attack or similar) from gaining access to sensitive files on your home directory or access to all of your SSH keys. Litterbox achieves file system isolation by restricting a development container to only have access to a single folder on your host system (and nothing above it). SSH key protection is achieved with a custom SSH agent that only exposes a limited number of SSH keys to a single Litterbox and prompts the user (via a pop-up window) before completing any signing requests.

N.B. Litterbox is free software that does not come with any warranty or guarantees. It is not an anti-malware solution and does not guarantee that your system will be protected from any attacks. Its goal is just to be BETTER THAN NOTHING but even that is not guaranteed. By using this software you agree that you are doing so at your own risk and take full responsibility for anything that might happen.

## Introductory presentation

For those who like videos, the following presentation provides a quick overview of Litterbox:

[![Litterbox Presentation](https://img.youtube.com/vi/MjEfGRDy-wA/maxresdefault.jpg)](https://www.youtube.com/watch?v=MjEfGRDy-wA)

## Isolation limitations

The isolation/sandboxing provided by Litterbox is limited and still leaves open many holes and/or vulnerabilities. It is not intented to shield you from software that is known to be malicious so please do not run malware or untrusted software inside it deliberatly. Its only goal is to try and provide SOME damange limitation in the event that 3rd party software and/or code that you trust were to unexpectedly get compromised.

By design, Litterbox comes with AT LEAST the following limitation/vulnerabilities:

- Everything running inside a Litterbox is running on top of your host kernel in the same way as normal applications. Thus, anything running inside the Litterbox could still exploit vulnerabilities in your host kernel to gain full access to your system.
- Everything running inside a Litterbox has full access to your Wayland server in the same way as normal applications. Thus, anything running inside the Litterbox could still exploit vulnerabilities in your Wayland server to gain full access to your system.
- Since applications running inside a Litterbox have normal access to your Wayland server, they have full access to things such as your clipboard so you should avoid copying any sensitive data around while you have a Litterbox running.
- If you enable PipeWire support, everything running inside a Litterbox has full access to your PipeWire server. Thus, anything running inside the Litterbox could exploit vulnerabilities in PipeWire to gain full access to your system. Additionallly, anything running inside the Litterbox can use PipeWire to record audio from your microphone or play audio through your speakers.
- Litterbox relies on Podman as its container runtime. Thus, anything running inside a Litterbox could still exploit vulnerabilities in your Podman engine to gain full access to your system.
- By default, Litterbox only provides limited network isolation. You should therefore be very careful to not have anything sensitive and/or vulnerable accessible on your network.
- Litterbox hosts an SSH agent server powered by [russh](https://crates.io/crates/russh). The goal of this server is to provide restricted access to SSH keys inside a Litterbox through a shared socket. Thus, anything running inside a Litterbox could still exploit vulnerabilities in this library to gain full access to your system.
- When you expose a device inside a Litterbox, you grant everything inside the Litterbox full access to that device.
- Currently Litterbox has many external dependencies which unfortunately makes Litterbox itself vulnerable to supply chain attacks. A long-term goal is thus to reduce the number of external dependencies to a bare minimum.

N.B. it is again emphasised that Litterbox does not come with any warranties or guarantees. Using it is at your own risk and the Litterbox authors do not accept any libiality for damages that might be incurred.

## Dependencies

Litterbox is a mostly static binary and only links to a few very standard shared libraries (that should exist on most Linux systems) as shown below:

```bash
gerhard@big-desktop:~$ ldd $(which litterbox)
	linux-vdso.so.1 (0x00007bef700a4000)
	libgcc_s.so.1 => /lib/x86_64-linux-gnu/libgcc_s.so.1 (0x00007bef70060000)
	libm.so.6 => /lib/x86_64-linux-gnu/libm.so.6 (0x00007bef6eef3000)
	libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007bef6ec00000)
	/lib64/ld-linux-x86-64.so.2 (0x00007bef700a6000)
```

It also depends on `podman` being installed on your system and the `mknod` command being available.

Hence, almost any modern Linux distro on which you can install `podman` should work. Litterbox also takes advantage of `Landlock` for added security on systems where it is available (i.e. most modern distros).

## Installation

By installing Litterbox you agree that you have read all the warnings above and that you are using it at your own risk.

To install Litterbox, simply run:

```bash
curl -fsSL https://litterbox.work/install.sh | sh
```

Unfortunately the installer currently only supports x86-64 binaries. If you are on a different platform, please build from source instead:

```bash
git clone https://github.com/Gerharddc/litterbox.git
cd litterbox
cargo build --release
```

## Usage

### 1. Define

First you will need to define your Litterbox by running `litterbox define LBX_NAME`. This will prompt you to pick a template and will place a Dockerfile in your `~/Litterbox/definitions` directory. The templates are a bit opinionated about what gets installed by default, so feel free to modify them! Please take note that (as described in the Dockerfile templates), anything you do inside the container's home directory during the image build phase will "disappear" when the container runs. This is because a different directory on your host (in `~/Litterbox/homes`) gets mounted over it at runtime. Thus, the Dockerfiles instead provide a script which gets run the first time that the container starts in order to set up the home directory.

### 2. Build

Then you will need to build your Litterbox by running `litterbox build LBX_NAME`. If you ever want to delete it again, simply run `litterbox delete LBX_NAME`. If you try to build a Litterbox that already exists, you will be offered the option to rebuild it or to do nothing.

During the build process, you will be asked various questions related to how you want to configure this Litterbox. These primarily concern which non-default access you want to give this Litterbox (such as wether it should have access to PipeWire). These settings are stored at `~/Litterbox/LBX_NAME.ron` and can be changed either by editing the file directly or by rebuilding the Litterbox and opting to change the settings. You will have to rebuild the Litterbox after changing the settings file for things to take effect though.

### 3. Enter

Finally you can then enter your Litterbox by running `litterbox enter LBX_NAME`. Once inside the Litterbox you can then start working on your projects! You can enter the same Litterbox multiple times from different terminals - all terminals share the same running container and this container will automatically stop when the last terminal exits.

### 4. Keys

If you want SSH keys to be available inside a Litterbox, simply run `litterbox keys generate KEY_NAME` to genererate a random key. You can then attach it to a Litterbox by running `litterbox keys attach KEY_NAME LBX_NAME` and detach it again using `litterbox keys attach KEY_NAME`. You can also view the public key by running `litterbox keys print KEY_NAME`. When a key is attached to a Litterbox, it is available through an SSH agent socket and each attempted interaction with the agent prompts a confirmation window to pop up. Also note that the keys are stored in `~/Litterbox/keys.ron` and encrypted with a password that you chose.

### 5. Devices

If you ever need to make a device (such as a virtual serial port) available inside a Litterbox, simply run `litterbox device LBX_NAME DEVICE_PATH`. This will make the device available inside the Litterbox by creating a device node inside its home directory. To remove the device again later, simply delete this file that got created. Please note that the device node corresponds to a device using its device number and not some higher level identifier. Thus, if you for instance unplug the device and plug in a new device of the same type, the device node will now point to the new device. So be careful what you expose inside the Litterbox!

## Comparison to alternatives

### Full Virtual Machine

Even though good isolation can be achieved using a virtual machine, the idea with Litterbox is to provide decent isolation coupled with more convenience and less overhead. Litterbox runs everything on top of your host Linux kernel (thereby reducing overhead) and inside a folder that exists directly on the host (thereby making it simpler to share files). Furthermore, Litterbox allows applications to connect directly to the Wayland server on your host system which means that applications running inside the Litterbox are graphically composed just like normal applications and seamlessly have access to things like your clipboard (so be careful what you put in there).

N.B. copy and pasting files to/from the Litterbox currently won't work as expected in many cases since file paths inside and outside the Litterbox are different. Copying data rather than paths should work as normal though.

### DevContainers

Litterbox is very similar to DevContainers in that is uses Dockerfiles and containers to create a repeatable and somewhat isolated environment for a development project. A drawback with DevContainers though is that they are intended to be driven by an IDE and therefore require deep IDE integration to work properly. In practice this means that they are only really useable through VSCode (and maybe a few others). Litterbox tries to take a much more flexible approach in that it encourages you to instead run your entire IDE inside the container together with you project(s). This has the advantage that your IDE needs no knowledge of Litterbox and that your host system is also isolated from the IDE and any extensions that might be running inside it. Furthermore, you can easily develop multiple projects together inside a single Litterbox if you like since there isn't the same strong connection between a single code repository and a single DevContainer.

### Distrobox

Litterbox is most similar to Distrobox in terms of its design and functionality. The primary difference is that Distrobox does not aim to provide any isolation/sandboxing at all whereas Litterbox has a strong emphasis on providing it. Distrobox avoids sandboxing in order to provide more seamless integration between applications running inside the Distrobox and the host system. It tries to solve the problem of running software intended for a different distro as if it is running natively. Litterbox instead sacrificies much of the convenience that Distrobox provides in exchange for some isolation/sandboxing capabilities.

## Stability

Litterbox is still early in its development lifecycle and not particularly stable yet. Litterbox does not follow proper "semver" yet since we want to reserve 1.0.0 to signify a decent level of stability. Currently, (in the context of a version specified as `x.y.z`), changes in `y` will indicate breaking changes. It is therefore important to keep track of release notes for breaking changes when updating as you might have to make changes to your Litterbox definitions and rebuild them.

Also keep in mind that the `main` branch might be broken from time to time because proper automated testing is not in place yet. Hence, it is recommended to also check out a tagged release if you want something that works. The releases are not guaranateed to be 100% working either, but more effort it made to test them.

## TODO

Litterbox is still very much WIP with many missing features or required improvements. Following is a list of some important pieces that are still missing:

- [ ] Improve documentation.
- [ ] Add proper automated testing.
- [x] Add function to change password for stored keys.
- [x] Add function to approve some SSH agent requests for the duration of the session.
- [x] Add optional support for using host network.
- [x] Add optional support for port forwarding with the default "pasta" networking.
- [x] Support entering a Litterbox multiple times.
- [x] Use `Landlock` to improve isolation strength.
- [x] Make it possible for X11 apps to run via `xwayland-satellite` integration.
- [ ] Add a "prune" command to get rid of dangling images.
- [ ] Add support for more granular network settings.
- [ ] Show SSH key name when prompting for approval. (Currently blocked by https://github.com/Eugeny/russh/issues/602)
- [ ] Expose limited DBus access to allow applications to open URLs. Likely using [dbus-proxy](https://github.com/Pelagicore/dbus-proxy).
- [ ] Add full support for running on Windows via WSL.
- [ ] Add Dockerfile templates for more distros.
- [ ] Add support for more hardware platforms to the installer.
- [ ] Release a version that uses Zenity for prompting for users that want a smaller binary.
- [ ] Try to provide a VM option using [crosvm's Wayland functionality](https://crosvm.dev/book/devices/wayland.html).

## Contributing

Litterbox already meets most of my own needs and I have higher priority projects that I currently want to focus on instead. Hence, I will unfortunately not be able to spend much time (if any) on feature requests and bug reports. However, I would be more than happy to accept help in the form of PRs. Also please feel free to help out in any other way you see suitable!
