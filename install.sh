#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="$HOME/.mlx-server"
LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
PLIST_FILE="$LAUNCH_AGENTS_DIR/ai.mlx.server.plist"

echo "=== MLX Dynamic Model Swapper Installer ==="

# 1. Ensure target directory exists
mkdir -p "$TARGET_DIR"
mkdir -p "$HOME/.local/bin"

# 2. Symlink mlx_manager.py
echo "[1/4] Linking mlx_manager.py to $TARGET_DIR/mlx_manager.py..."
ln -sf "$SCRIPT_DIR/mlx_manager.py" "$TARGET_DIR/mlx_manager.py"

# 3. Install CLI tool
echo "[2/4] Installing mlx CLI to $HOME/.local/bin/mlx..."
ln -sf "$SCRIPT_DIR/bin/mlx" "$HOME/.local/bin/mlx"
chmod +x "$HOME/.local/bin/mlx"

# 4. Generate and copy LaunchAgent plist
echo "[3/4] Configuring macOS LaunchAgent..."
mkdir -p "$LAUNCH_AGENTS_DIR"
sed "s|{{HOME}}|$HOME|g" "$SCRIPT_DIR/ai.mlx.server.plist.template" > "$PLIST_FILE"

# 5. Load LaunchAgent
echo "[4/4] Starting service via launchctl..."
launchctl unload "$PLIST_FILE" 2>/dev/null || true
launchctl load "$PLIST_FILE"

echo "✔ Installation complete!"
echo "Check status with: mlx status"
echo "View logs with:   mlx logs"
