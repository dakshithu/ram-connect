<div align="center">

# 🧠 RamConnect

**Turn spare RAM on any machine into shared swap for another.**

Pool memory across your devices over the LAN — Windows, Linux, and macOS.

![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)
![Architecture](https://img.shields.io/badge/arch-x86__64-lightgrey)
![Status](https://img.shields.io/badge/status-active-brightgreen)

</div>

---

## 📖 Overview

RamConnect lets one device (the **Contributor**) share a portion of its RAM over the network with another device (the **Organizer**), which mounts it as usable swap. Think of it as **network-attached memory** — turning idle RAM on your other machines into extra headroom for the device that needs it.

---

## ✨ Features

- 🔄 Cross-platform support — Windows, Linux, macOS
- ⚡ Low-latency RAM streaming over LAN
- 🖥️ Web dashboard for monitoring nodes
- 🔌 Simple Organizer / Contributor node model
- 🛠️ Optional native swap integration on Linux

---

## 🖥️ Platform Requirements

### Windows

| Category | Details |
|---|---|
| **Supported OS** | Windows 10, Windows 11, Windows Server 2016+ (64-bit x86_64) |
| **Default Mount / Storage Path** | Virtual Drive `R:\` |
| **Required System Utilities** | Standard Windows Shell / PowerShell |

### Linux

| Category | Details |
|---|---|
| **Supported OS** | Any modern 64-bit distro (Ubuntu, Debian, Fedora, Arch, Alpine, etc.) with Kernel 3.10+ |
| **Default Mount / Storage Path** | `/mnt/ramconnect` (tmpfs) |
| **Required System Utilities** | `tmpfs`, `mount`, `swapon` / `swapoff` (optional, for swap integration) |

---

## 🔐 Permissions

| Platform | Requirement |
|---|---|
| **Windows** | Runs as a standard user portable binary (`.exe`). Administrator rights are only needed to configure custom Windows Firewall rules. |
| **Linux** | Basic node operation runs under standard user privileges. Root (`sudo`) is required for automatic tmpfs RAM drive mounting (`/mnt/ramconnect`) or Linux virtual swap file management (`swapon`/`swapoff`). |

---

## 🌐 Network & Firewall Requirements

Both Windows Firewall and Linux firewalls (`ufw`, `firewalld`, `iptables`) must allow **inbound and outbound** traffic on the following ports.

### Organizer Node (Control Plane & Web UI)

| Protocol | Port | Purpose |
|---|---|---|
| TCP | `8080` | Default Web Dashboard & API port (fallback: `8081`, `8082`, `3000`) |
| UDP | `5353` | Multicast DNS (mDNS) peer discovery across the LAN |

### Contributor Node (Memory Provider)

| Protocol | Port | Purpose |
|---|---|---|
| TCP | `9000` | Default RAM payload transfer socket |
| TCP | `9190` | Local node web dashboard & HTTP status |

> **💡 Network Recommendation:** Use 1 Gbps+ Wired Ethernet or 5 GHz / Wi-Fi 6 for low-latency RAM streaming across nodes.
>
> **Note:** You can change the default ports if you want as well!

---

## 🧩 Hardware & Resource Requirements

**CPU:** x86_64 (64-bit). A multi-core processor is recommended for concurrent socket streaming.

**RAM:**

| Node Type | Usage |
|---|---|
| **Organizer Node** | Minimal resource usage (~30–50 MB RAM for the control plane) |
| **Contributor Node** | Minimal host overhead, plus whatever RAM you choose to share with the mesh network (e.g., 512 MB – 64+ GB) |

---

## 🚀 Getting Started

### Organizer Node

- Run with default web port (`8080`):

  ```bash
  cargo run --bin organizer
  ```

- Run with a custom web port (e.g. `8085`):

  ```bash
  cargo run --bin organizer -- 8085
  ```

- Run with release optimizations (recommended for performance):

  ```bash
  cargo run --release --bin organizer
  ```

### Contributor Node

- Run with default ports (`TCP: 9000`, `Web: 9190`):

  ```bash
  cargo run --bin contributor
  ```

- Run with custom TCP & Web ports (e.g. `TCP: 9001`, `Web: 9191`):

  ```bash
  cargo run --bin contributor -- 9001 9191
  ```

- Run with release optimizations:

  ```bash
  cargo run --release --bin contributor
  ```

### Building Standalone Binaries

If you just want to compile the standalone binary files:

```bash
cargo build --release --bins
```

The compiled binaries will be located at:

- **Windows:** `target\release\organizer.exe` and `target\release\contributor.exe`
- **Linux:** `target/release/organizer` and `target/release/contributor`

---

## 🗺️ Roadmap

- [ ] macOS setup documentation
- [ ] Encrypted transport for RAM payloads
- [ ] Automatic node discovery UI
- [ ] Bandwidth throttling controls

---

## 📄 License

_Add your license here._
