#!/bin/bash
# Cross-Platform Bash Release Packaging Script for RAM Connect (Linux & macOS)
set -e

echo -e "\033[36mCreating RAM Connect Distribution Packages...\033[0m"

DIST_WIN="dist/windows"
DIST_LINUX="dist/linux"
DIST_MAC="dist/macos"

mkdir -p "$DIST_WIN" "$DIST_LINUX" "$DIST_MAC"

# Build local release binaries if cargo is available
if command -v cargo &> /dev/null; then
    echo -e "\033[33mBuilding release binaries via cargo...\033[0m"
    cargo build --release --bins
fi

# Detect platform and copy binaries if present
UNAME_S=$(uname -s)
if [ -f "target/release/organizer" ]; then
    if [ "$UNAME_S" = "Darwin" ]; then
        cp "target/release/organizer" "$DIST_MAC/organizer"
        cp "target/release/contributor" "$DIST_MAC/contributor"
        chmod +x "$DIST_MAC/organizer" "$DIST_MAC/contributor"
        echo -e "\033[32mCopied macOS release binaries to $DIST_MAC\033[0m"
    else
        cp "target/release/organizer" "$DIST_LINUX/organizer"
        cp "target/release/contributor" "$DIST_LINUX/contributor"
        chmod +x "$DIST_LINUX/organizer" "$DIST_LINUX/contributor"
        echo -e "\033[32mCopied Linux release binaries to $DIST_LINUX\033[0m"
    fi
fi

# Create macOS Launchers
cat << 'EOF' > "$DIST_MAC/Start-Organizer.command"
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Organizer..."
chmod +x ./organizer
./organizer "$@"
EOF
chmod +x "$DIST_MAC/Start-Organizer.command"

cat << 'EOF' > "$DIST_MAC/Start-Contributor.command"
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Contributor..."
chmod +x ./contributor
./contributor "$@"
EOF
chmod +x "$DIST_MAC/Start-Contributor.command"

# Create Linux Launchers
cat << 'EOF' > "$DIST_LINUX/Start-Organizer.sh"
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Organizer..."
chmod +x ./organizer
./organizer "$@"
EOF
chmod +x "$DIST_LINUX/Start-Organizer.sh"

cat << 'EOF' > "$DIST_LINUX/Start-Contributor.sh"
#!/bin/bash
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"
echo "Starting RAM Connect Contributor..."
chmod +x ./contributor
./contributor "$@"
EOF
chmod +x "$DIST_LINUX/Start-Contributor.sh"

echo -e "\033[36mPackage distribution files successfully prepared in dist/\033[0m"
