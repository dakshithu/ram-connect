<div align="center">

# 🧠 RamConnect

**Turn spare RAM on any machine into shared swap for another.**

Pool memory across your devices over the LAN — Windows, Linux, and macOS.

![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)
![Architecture](https://img.shields.io/badge/arch-x86__64%20%7C%20ARM64-lightgrey)
![License](https://img.shields.io/badge/license-AGPLv3-orange)
![Status](https://img.shields.io/badge/status-active-brightgreen)

</div>

---

## 📖 Overview

RamConnect lets one device (the **Contributor**) share a portion of its RAM over the network with another device (the **Organizer**), which mounts it as usable swap. Think of it as **network-attached memory** — turning idle RAM on your other machines into extra headroom for the device that needs it.

---

## ✨ Features

- 🔄 Cross-platform support — Windows, Linux, macOS (Intel & Apple Silicon)
- ⚡ Low-latency RAM streaming over LAN
- 🖥️ Web dashboard for monitoring nodes
- 🔌 Simple Organizer / Contributor node model
- 🛠️ Native swap and drive integration (tmpfs on Linux, WebDAV on Windows/macOS)

---

## 🌐 1. General & Hardware Requirements

- **Architecture:** 64-bit CPU (`x86_64` for Windows/Linux/macOS, `aarch64` / ARM64 for Apple Silicon). Multi-core recommended for socket streaming.
- **RAM Overhead:**
  - **Organizer Node:** Minimal (~30–50 MB RAM for the control plane).
  - **Contributor Node:** Minimal host overhead + allocated RAM pool size (e.g., 512 MB to 64+ GB).
- **Network & Connectivity:** Low-latency LAN connection (1 Gbps+ Wired Ethernet or 5 GHz / Wi-Fi 6 recommended). Multicast DNS (mDNS) enabled on the local network for automatic peer discovery.
- **Build Requirements (For compiling from source):** Rust Toolchain (`rustc` / `cargo`, Rust 2021 edition, MSRV 1.75+).

---

## 🖥️ 2. Platform Requirements & Storage Integration

### 🪟 Windows

| Category | Details |
|---|---|
| **Supported OS** | Windows 10, Windows 11, Windows Server 2016+ (64-bit `x86_64`) |
| **Permissions** | Standard user privileges for portable binary execution (`organizer.exe`, `contributor.exe`). Administrator rights required only for Windows Firewall rules or system pagefile configuration. |
| **System Utilities & Services** | • Windows WebClient Service (`sc start WebClient` or `net start WebClient`) and `net use` command for mounting WebDAV mesh storage.<br>• PowerShell or Command Prompt (`cmd.exe`).<br>• `wmic` or PowerShell (required if using the RAM mesh as Windows Virtual Swap `R:\pagefile.sys`). |
| **Default Storage Mount** | Mapped Virtual System Drive `R:\` (via `\\127.0.0.1@<port>\dav` or `http://127.0.0.1:<port>/dav`) |

### 🐧 Linux

| Category | Details |
|---|---|
| **Supported OS** | Any modern 64-bit Linux distribution (Ubuntu, Debian, Fedora, Arch, Alpine, etc.) running Kernel 3.10+ |
| **Permissions** | Basic node execution runs as a standard user. Root (`sudo`) privileges are required for auto-mounting `tmpfs` drives and managing kernel swap files (`swapon`/`swapoff`). |
| **System Utilities & Tools** | • `tmpfs` kernel filesystem support.<br>• Filesystem utilities: `mount`, `umount`, `chmod`, `dd` or `fallocate`.<br>• Swap utilities: `mkswap`, `swapon`, `swapoff`.<br>• POSIX Shell (`/bin/bash` or `/bin/sh` for launcher scripts). |
| **Build Dependencies (Static)** | Docker / Alpine Linux (if building statically linked `musl` binaries via Dockerfile). Target triple: `x86_64-unknown-linux-musl`. |
| **Default Storage Mount** | `/mnt/ramconnect` (`tmpfs`) and `/var/ramconnect/ram_swap.img` (for virtual swap memory) |

### 🍏 macOS

| Category | Details |
|---|---|
| **Supported OS / Hardware** | macOS 10.15+ on Apple Silicon (`aarch64-apple-darwin` M1/M2/M3/M4) or Intel (`x86_64-apple-darwin`). Universal binaries supported via `lipo`. |
| **Permissions** | Standard user privileges. No root access needed for mounting Finder WebDAV volumes. |
| **System Utilities & Built-ins** | • `mount_webdav` binary (built-in macOS WebDAV mount tool).<br>• macOS Finder & `osascript` (AppleScript support for mounting `webdav://127.0.0.1:<port>/dav`).<br>• System tools: `open`, `umount`, `diskutil`.<br>• macOS Terminal / `/bin/bash` (for `.command` launcher scripts). |
| **Build Dependencies (Cross)** | • `rustup target add aarch64-apple-darwin x86_64-apple-darwin`<br>• `lipo` utility (for combining Apple Silicon and Intel into a Universal Binary)<br>• `osxcross` (optional, for Docker-based macOS cross-compilation) |
| **Default Storage Mount** | `~/RAMConnect_Drive` or `/Volumes/RAMConnect` (mounted natively via WebDAV into macOS Finder) |

---

## 🔐 3. Network & Firewall Requirements

Both host firewalls (Windows Firewall, Linux `ufw`/`iptables`, macOS Firewall) must allow **inbound and outbound** traffic on the following ports:

### Organizer Node (Control Plane & Web UI)

| Protocol | Port | Purpose |
|---|---|---|
| TCP | `8080` | Default Web Dashboard & API port (fallbacks: `8081`, `8082`, `3000`) |
| UDP | `5353` | Multicast DNS (mDNS) peer discovery across the LAN |

### Contributor Node (Memory Provider)

| Protocol | Port | Purpose |
|---|---|---|
| TCP | `9000` | Default RAM payload transfer socket |
| TCP | `9190` | Local node web dashboard & HTTP status |

> **💡 Note:** All default ports are configurable via command-line arguments.

---

## 🚀 4. Getting Started

### Running the Organizer Node

- **Default Web Port (`8080`):**
  ```bash
  cargo run --bin organizer
  ```
- **Custom Web Port (e.g. `8085`):**
  ```bash
  cargo run --bin organizer -- 8085
  ```
- **Release Mode (Recommended for performance):**
  ```bash
  cargo run --release --bin organizer
  ```

### Running the Contributor Node

- **Default Ports (`TCP: 9000`, `Web: 9190`):**
  ```bash
  cargo run --bin contributor
  ```
- **Custom TCP & Web Ports (e.g. `TCP: 9001`, `Web: 9191`):**
  ```bash
  cargo run --bin contributor -- 9001 9191
  ```
- **Release Mode:**
  ```bash
  cargo run --release --bin contributor
  ```

---

## 📦 5. Building Standalone Binaries

Compile standalone release binaries using Cargo:

```bash
cargo build --release --bins
```

The compiled binaries will be available in:

- **Windows:** `target\release\organizer.exe` and `target\release\contributor.exe`
- **Linux:** `target/release/organizer` and `target/release/contributor`
- **macOS:** `target/release/organizer` and `target/release/contributor`

### Building Universal Binaries for macOS (Intel + Apple Silicon)

```bash
rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin --bins
cargo build --release --target aarch64-apple-darwin --bins

# Combine into Universal Binaries
lipo -create -output target/release/organizer \
  target/x86_64-apple-darwin/release/organizer \
  target/aarch64-apple-darwin/release/organizer

lipo -create -output target/release/contributor \
  target/x86_64-apple-darwin/release/contributor \
  target/aarch64-apple-darwin/release/contributor
```

---

## 🗺️ Roadmap

- [x] macOS setup documentation and native Finder WebDAV integration
- [ ] Encrypted transport for RAM payloads
- [ ] Automatic node discovery UI
- [ ] Bandwidth throttling controls

---

## 📄 License

This project is licensed under the **GNU Affero General Public License v3.0** (AGPLv3).

```text
GNU AFFERO GENERAL PUBLIC LICENSE
Version 3, 19 November 2007

Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>
Everyone is permitted to copy and distribute verbatim copies
of this license document, but changing it is not allowed.
```

For full details, see the [LICENSE](LICENSE) file in the repository root.
