<div align="center">

# 🧠 RamConnect

**Turn spare RAM on any machine into shared swap for another.**

Pool memory across your devices over the LAN — Windows, Linux, and macOS.

![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)
![Architecture](https://img.shields.io/badge/arch-x86__64%20%7C%20ARM64-lightgrey)
![License](https://img.shields.io/badge/license-AGPLv3-orange)
![Status](https://img.shields.io/badge/status-active-brightgreen)
![Views](https://komarev.com/ghpvc/?username=dakshithu&repo=ram-connect&color=blue&label=Repo+Views)

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
- 🛠️ Native swap and drive integration (`tmpfs` on Linux, WebDAV on Windows/macOS)

---

## 📊 Repo Stats

![Views](https://komarev.com/ghpvc/?username=dakshithu&repo=ram-connect&color=blue&label=Total+Views)
![Stars](https://img.shields.io/github/stars/dakshithu/ram-connect?style=social)
![Forks](https://img.shields.io/github/forks/dakshithu/ram-connect?style=social)

> **Note on view tracking:** GitHub doesn't expose a public, lifetime view count for any repo — its native **Insights → Traffic** page only shows the last 14 days, and only to repo owners/collaborators. The counter above is a free, self-hosted badge ([komarev.com/ghpvc](https://github.com/antonkomarev/github-profile-views-counter)) that increments every time this README is loaded and persists indefinitely, so it counts every view going forward from the moment you add it. There's no way to backfill views from before it's added — GitHub never stored that data publicly to begin with, so "from the start" is only possible if the badge is in place before people start visiting.

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
| **System Utilities & Services** | • Windows WebClient Service (`sc start WebClient` or `net start WebClient`) and `net use` command for mounting WebDAV mesh storage.<br>• PowerShell or Command Prompt (`cmd.exe`).<br>• PowerShell (required if using the RAM mesh as Windows Virtual Swap `R:\pagefile.sys`). |
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

## 🛠️ 4. Building from Source

Ensure you have Rust installed (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` or via standard package manager).

### Quick Build (Native Platform)

Compile optimized release binaries for your current operating system:

```bash
cargo build --release --bins
```

The output binaries will be generated in `target/release/`:
- **Windows:** `target\release\organizer.exe` and `target\release\contributor.exe`
- **Linux:** `target/release/organizer` and `target/release/contributor`
- **macOS:** `target/release/organizer` and `target/release/contributor`

---

### Platform-Specific & Cross-Platform Builds

#### 🪟 Windows Build
```powershell
# Native build in PowerShell or CMD
cargo build --release --bins

# Package portable release zip / folder layout
.\package_release.ps1
```

#### 🐧 Linux Build

- **Native Linux Build:**
  ```bash
  cargo build --release --bins
  ```

- **Static Linux Binary Build (Docker / musl):**
  Generate zero-dependency, statically-linked binaries suitable for any Linux distro (including minimal/Alpine hosts):
  ```bash
  docker build -t ram-connect-linux -f Dockerfile .
  docker run --rm -v $(pwd)/dist/linux:/output ram-connect-linux
  ```

- **Linux Release Packaging:**
  ```bash
  chmod +x package_release.sh
  ./package_release.sh
  ```

#### 🍏 macOS Build (Intel & Apple Silicon)

- **Native Build (Current Architecture):**
  ```bash
  cargo build --release --bins
  ```

- **Universal macOS Binary (Apple Silicon ARM64 + Intel x86_64):**
  To create a single binary set that runs natively on both M-series and Intel Macs:
  ```bash
  # 1. Add target toolchains
  rustup target add x86_64-apple-darwin aarch64-apple-darwin

  # 2. Build for both architectures
  cargo build --release --target x86_64-apple-darwin --bins
  cargo build --release --target aarch64-apple-darwin --bins

  # 3. Combine using lipo
  lipo -create -output target/release/organizer \
    target/x86_64-apple-darwin/release/organizer \
    target/aarch64-apple-darwin/release/organizer

  lipo -create -output target/release/contributor \
    target/x86_64-apple-darwin/release/contributor \
    target/aarch64-apple-darwin/release/contributor
  ```

---

## 🚀 5. Getting Started & Running Nodes

### 🪟 Windows

1. **Launcher Scripts:** Double-click `dist\windows\Start-Organizer.bat` or `dist\windows\Start-Contributor.bat`.
2. **Command Line:**
   ```cmd
   # Run Organizer on default port (8080)
   target\release\organizer.exe

   # Run Contributor on default TCP (9000) & Web (9190)
   target\release\contributor.exe
   ```
3. **Mounting Storage:** Access `http://127.0.0.1:8080` in your browser and click **Auto-Mount Mesh**. Ensure the Windows `WebClient` service is active (`net start WebClient`).

### 🐧 Linux

1. **Launcher Scripts:**
   ```bash
   cd dist/linux
   chmod +x Start-Organizer.sh Start-Contributor.sh organizer contributor
   ./Start-Organizer.sh
   # On donor machine:
   ./Start-Contributor.sh
   ```
2. **Command Line:**
   ```bash
   ./target/release/organizer 8080
   ./target/release/contributor 9000 9190
   ```
3. **Mounting Swap:** Use `sudo` privileges when prompted by the Organizer to mount `tmpfs` at `/mnt/ramconnect` or create `/var/ramconnect/ram_swap.img` with `swapon`.

### 🍏 macOS

1. **Launcher Scripts:** Double-click `dist/macos/Start-Organizer.command` or `dist/macos/Start-Contributor.command` in Finder.
2. **Terminal:**
   ```bash
   chmod +x target/release/organizer target/release/contributor
   ./target/release/organizer 8080
   ./target/release/contributor 9000 9190
   ```
3. **Mounting Drive:** In the web UI (`http://127.0.0.1:8080`), click **Auto-Mount Mesh as Physical System Drive**, or open `webdav://127.0.0.1:8080/dav` in Finder to mount under `/Volumes/RAMConnect`.

---

## ❓ 6. Troubleshooting & FAQs

### 🌐 Network & Discovery Issues

- **Nodes not discovering each other automatically:**
  - Verify both machines are on the same local subnet / LAN.
  - Ensure Multicast DNS (mDNS) port `UDP 5353` is allowed by your firewall.
  - If mDNS is blocked by your router/AP, manually specify the Contributor IP address in the Organizer Web Dashboard.
- **Connection Refused or Firewall Blocks:**
  - Check that TCP ports `8080` (Organizer Web), `9000` (Contributor Payload), and `9190` (Contributor Web) are open in host firewalls (`ufw allow 8080/tcp`, Windows Firewall inbound rules, macOS Security Settings).

### 🪟 Windows WebDAV / Mount Troubleshooting

- **Error: "The network path was not found" or WebDAV cannot mount `R:\`:**
  - The Windows WebClient service might be stopped. Start it by running CMD/PowerShell as Administrator:
    ```cmd
    net start WebClient
    ```
  - Set WebClient to automatic start:
    ```cmd
    sc config WebClient start= auto
    ```
- **WebDAV file size transfer limits:**
  - Windows defaults to a 50MB file size limit for WebDAV streams. If transferring large virtual swap files over WebDAV, increase `FileSizeLimitInBytes` under `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\WebClient\Parameters` in Regedit.

### 🐧 Linux Mount & Permission Errors

- **`Permission denied` when mounting `/mnt/ramconnect` or creating swap:**
  - Mounting `tmpfs` and enabling swap via `swapon` requires `root` / `sudo` privileges. Run the Organizer binary or script with `sudo` if auto-mount fails.
- **`swapon: /var/ramconnect/ram_swap.img: read swap header failed`:**
  - Ensure the swap image was properly formatted with `mkswap` before `swapon`. Delete the corrupt file and let RamConnect re-initialize the pool:
    ```bash
    sudo swapoff /var/ramconnect/ram_swap.img 2>/dev/null
    sudo rm -f /var/ramconnect/ram_swap.img
    ```

### 🍏 macOS Execution & Finder Mount Issues

- **macOS Security Warning: `"organizer" cannot be opened because it is from an unidentified developer`:**
  - Open **System Settings > Privacy & Security** and click **Allow Anyway**, or remove the quarantine attribute via Terminal:
    ```bash
    xattr -d com.apple.quarantine target/release/organizer target/release/contributor
    ```
- **Finder WebDAV fails to mount `/Volumes/RAMConnect`:**
  - Check if port 8080 (or configured Web port) is in use by another app.
  - Test mounting manually from Finder: Press `Cmd + K`, enter `http://127.0.0.1:8080/dav` (or `webdav://127.0.0.1:8080/dav`), and connect as **Guest**.

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
