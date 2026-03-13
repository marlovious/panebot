#!/bin/bash

echo "👋 Welcome to Marlovious PaneBot Setup."

CONFIG_DIR="$HOME/.config/panebot"
PANES_CONF="$CONFIG_DIR/panes.conf"

# Determine install directory based on OS
if [[ "$(uname)" == "Darwin" ]]; then
    INSTALL_DIR="$HOME/bin"
else
    INSTALL_DIR="$HOME/.local/bin"
fi

mkdir -p "$INSTALL_DIR"
mkdir -p "$CONFIG_DIR/sockets"

SRC_MPVC="./panebot.mpvc.sh"
TARGET_MPVC="$INSTALL_DIR/panebot.mpvc.sh"

if [[ ! -x "$TARGET_MPVC" ]]; then
    if [[ -f "$SRC_MPVC" ]]; then
        cp "$SRC_MPVC" "$TARGET_MPVC"
        chmod +x "$TARGET_MPVC"
        echo "✅ Installed panebot.mpvc.sh to $TARGET_MPVC"
    else
        echo "❌ Could not find panebot.mpvc.sh in current directory."
        exit 1
    fi
else
    echo "ℹ️ panebot.mpvc.sh already installed at $TARGET_MPVC"
fi

PANEBOT_MPVC="$TARGET_MPVC"

generate_wrapper() {
    local name="$1"
    local geometry="$2"
    local socket="$CONFIG_DIR/sockets/$name"

    mkdir -p "$(dirname "$socket")"

    local wrapper="$INSTALL_DIR/$name"
    cat > "$wrapper" <<EOF
#!/bin/bash
MPVC_SOCKET="$socket"
exec "$PANEBOT_MPVC" --socket="\$MPVC_SOCKET" --geometry="$geometry" "\$@"
EOF
    chmod +x "$wrapper"
    echo "✅ Created launcher: $wrapper"
}

read -p "How many video panes would you like to set up? " PANE_COUNT

CURRENT_Y=0
PADDING=10

for ((i=1; i<=PANE_COUNT; i++)); do
    echo ""
    echo "🔧 Configuring Pane #$i"

    read -p "Pane $i > Name this video pane (no spaces): " SOCKET_NAME
    read -p "$SOCKET_NAME > Specify Aspect Ratio (1:1,4:3,16:9,2.35:1) [ENTER for dynamic]: " ASPECT

    WIDTH=""
    HEIGHT=""

    if [[ -n "$ASPECT" ]]; then
        read -p "$SOCKET_NAME > Specify Width (pixels): " WIDTH
        IFS=":" read -r W H <<< "$ASPECT"
        HEIGHT=$(printf "%.0f" "$(echo "$WIDTH * $H / $W" | bc -l)")
    fi

    read -p "$SOCKET_NAME > Auto-place this pane based on previous layout? (Y/n): " AUTO_PLACE
    AUTO_PLACE=${AUTO_PLACE:-Y

