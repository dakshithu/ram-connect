# PowerShell Release Packaging Script for RAM Connect
$ErrorActionPreference = "Stop"

Write-Host "Creating RAM Connect Distribution Packages..." -ForegroundColor Cyan

# Create dist directories
$distWin = "dist\windows"
$distLinux = "dist\linux"
$distMac = "dist\macos"

New-Item -ItemType Directory -Force -Path $distWin | Out-Null
New-Item -ItemType Directory -Force -Path $distLinux | Out-Null
New-Item -ItemType Directory -Force -Path $distMac | Out-Null

# Copy Windows Executables if built
if (Test-Path "target\release\organizer.exe") {
    Copy-Item "target\release\organizer.exe" "$distWin\organizer.exe" -Force
    Copy-Item "target\release\contributor.exe" "$distWin\contributor.exe" -Force
    Write-Host "Copied Windows release binaries (organizer.exe, contributor.exe)" -ForegroundColor Green
} else {
    Write-Host "Release binaries not found in target\release yet." -ForegroundColor Yellow
}

# Create Windows Batch Launchers
$batOrg = @"
@echo off
title RAM Connect Organizer Control Plane
echo Starting RAM Connect Organizer...
echo Open dashboard in browser when port is displayed.
organizer.exe %*
pause
"@
Set-Content -Path "$distWin\Start-Organizer.bat" -Value $batOrg

$batContrib = @"
@echo off
title RAM Connect Contributor Node
echo Starting RAM Connect Contributor...
echo Open dashboard in browser when port is displayed.
contributor.exe %*
pause
"@
Set-Content -Path "$distWin\Start-Contributor.bat" -Value $batContrib

# Create Windows Readme
$readmeWin = @"
========================================================================
                      RAM Connect Portable - Windows
========================================================================

Portability Notice:
- These .exe files are fully portable, self-contained native Windows binaries.
- No installation, admin privileges, or Rust runtime required!

How to Run:
1. Double-click Start-Organizer.bat (or run organizer.exe) to host a RAM mesh.
2. Double-click Start-Contributor.bat (or run contributor.exe) on any connected PC to share RAM.

Command Line Options:
- organizer.exe [web_port]       (Default port: 8080)
- contributor.exe [tcp_port]     (Default TCP: 9000, Web: 9190)
"@
Set-Content -Path "$distWin\README.txt" -Value $readmeWin

# Create Linux Launcher & Readme
$shOrg = @"
#!/bin/bash
echo "Starting RAM Connect Organizer..."
chmod +x ./organizer
./organizer "$@"
"@
Set-Content -Path "$distLinux\Start-Organizer.sh" -Value $shOrg

$shContrib = @"
#!/bin/bash
echo "Starting RAM Connect Contributor..."
chmod +x ./contributor
./contributor "$@"
"@
Set-Content -Path "$distLinux\Start-Contributor.sh" -Value $shContrib

$readmeLinux = @"
========================================================================
                      RAM Connect Portable - Linux
========================================================================

Building Static Linux Binaries (No Rust or Dependencies Needed on Target PC):

Option 1: Build static binaries using Docker (Recommended)
  docker build -t ram-connect-linux .
  docker run --rm -v $(pwd)/dist/linux:/output ram-connect-linux

Option 2: Native build on Linux
  cargo build --release --bins

How to Run on Linux:
  chmod +x ./organizer ./contributor
  ./organizer 8080
  ./contributor 9000
"@
Set-Content -Path "$distLinux\README.txt" -Value $readmeLinux

# Create macOS Launchers & Readme
if (Test-Path "target\release\organizer") {
    Copy-Item "target\release\organizer" "$distMac\organizer" -Force
    Copy-Item "target\release\contributor" "$distMac\contributor" -Force
    Write-Host "Copied macOS release binaries (organizer, contributor)" -ForegroundColor Green
}

$cmdOrg = @'
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Organizer..."
chmod +x ./organizer
./organizer "$@"
'@
Set-Content -Path "$distMac\Start-Organizer.command" -Value $cmdOrg

$cmdContrib = @'
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Contributor..."
chmod +x ./contributor
./contributor "$@"
'@
Set-Content -Path "$distMac\Start-Contributor.command" -Value $cmdContrib

$readmeMac = @"
========================================================================
                      RAM Connect Portable - macOS
========================================================================

Overview:
  RAM Connect enables high-performance RAM sharing across local networks.
  On macOS, RAM Connect integrates natively with macOS Finder via WebDAV,
  mounting distributed RAM storage directly under /Volumes/RAMConnect.

========================================================================
1. How to Build Native macOS Binaries
========================================================================

Build natively on macOS (Apple Silicon M1/M2/M3/M4 or Intel):
   cargo build --release --bins

Option A: Build for Apple Silicon (ARM64):
   cargo build --release --target aarch64-apple-darwin --bins

Option B: Build for Intel Macs (x86_64):
   cargo build --release --target x86_64-apple-darwin --bins

Option C: Create Universal macOS Binary (Apple Silicon + Intel):
   lipo -create -output organizer target/aarch64-apple-darwin/release/organizer target/x86_64-apple-darwin/release/organizer
   lipo -create -output contributor target/aarch64-apple-darwin/release/contributor target/x86_64-apple-darwin/release/contributor

========================================================================
2. How to Run on macOS
========================================================================

Method 1: Double-Click Launcher Scripts
   - Double-click `Start-Organizer.command` to start the Organizer mesh server.
   - Double-click `Start-Contributor.command` to start a Contributor RAM node.

Method 2: Run via macOS Terminal
   chmod +x ./organizer ./contributor
   ./organizer 8080
   ./contributor 9000

========================================================================
3. macOS Finder Integration
========================================================================

- Click "⚡ Auto-Mount Mesh as Physical System Drive" in the Web Dashboard.
- Finder will mount the mesh volume under `/Volumes/RAMConnect`.
- Or run in Terminal / Safari:
    open http://127.0.0.1:8080/dav
========================================================================
"@
Set-Content -Path "$distMac\README.txt" -Value $readmeMac

Write-Host "Package distribution files prepared in dist/" -ForegroundColor Cyan
