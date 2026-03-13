#!/bin/bash

echo "👋 Welcome to PaneBot!"

read -p "How many video panes would you like to set up? " PANE_COUNT

SKEL="./skel.sh"
[[ ! -f "$SKEL" ]] && echo "❌ Missing skel.sh in current directory." && exit 1

# Determine install dir
if [[ -d "$HOME/.local/bin" ]]; then
    INSTALL_DIR="$HOME/.local/bin"
else
    INSTALL_DIR="$HOME/bin"
    mkdir -p "$INSTALL_DIR"
fi

CONFIG_DIR="$HOME/.config/panebot"
mkdir -p "$CONFIG_DIR"

# sed -i wrapper for GNU/BSD compatibility
if sed --version >/dev/null 2>&1; then
    SED_INPLACE() { sed -i "$@"; }
else
    SED_INPLACE() { sed -i '' "$@"; }
fi

# Offset tracking
CURRENT_X=0
CURRENT_Y=0
PADDING=10

SCRIPT_NAMES=()
PANE_MAP="$CONFIG_DIR/pane_map.txt"
touch "$PANE_MAP"

for (( i=1; i<=PANE_COUNT; i++ )); do
    echo ""
    echo "🔧 Configuring Pane #$i"

    read -p "Pane $i > Name this video pane (No Spaces): " SOCKET_NAME

    read -p "$SOCKET_NAME > Is this a remote pane? (y/N): " IS_REMOTE
    IS_REMOTE=$(echo "$IS_REMOTE" | tr '[:upper:]' '[:lower:]')

    if [[ "$IS_REMOTE" == "y" ]]; then
        read -p "$SOCKET_NAME > Enter remote hostname (user@host): " REMOTE_HOST
        read -p "$SOCKET_NAME > Install path on remote host (absolute, e.g. /home/user/.local/bin): " REMOTE_INSTALL_DIR
        read -p "$SOCKET_NAME > Use SSH key for passwordless login? (Y/n): " USE_KEY
        USE_KEY=$(echo "$USE_KEY" | tr '[:upper:]' '[:lower:]')

        if [[ "$USE_KEY" != "n" ]]; then
            KEY="$HOME/.ssh/id_rsa"
            if [[ ! -f "$KEY" ]]; then
                echo "🔑 SSH key not found. Generating..."
                ssh-keygen -t rsa -b 2048 -f "$KEY"
            fi
            echo "🚀 Copying key to $REMOTE_HOST"
            ssh-copy-id "$REMOTE_HOST"
        fi
    fi

    read -p "$SOCKET_NAME > Specify Aspect Ratio (1:1, 4:3, 16:9, 2.35:1) [ENTER for dynamic]: " ASPECT

    if [[ -n "$ASPECT" ]]; then
        read -p "$SOCKET_NAME > Specify Width: " WIDTH
    fi

    read -p "$SOCKET_NAME > Auto-place this pane based on previous layout? (Y/n): " AUTO_PLACE
    AUTO_PLACE=${AUTO_PLACE:-Y}
    AUTO_PLACE=$(echo "$AUTO_PLACE" | tr '[:upper:]' '[:lower:]')

    if [[ "$AUTO_PLACE" == "y" ]]; then
        if [[ -n "$ASPECT" && -n "$WIDTH" ]]; then
            IFS=":" read -r W H <<< "$ASPECT"
            HEIGHT=$(printf "%.0f" "$(echo "$WIDTH * $H / $W" | bc -l)")
            GEOMETRY="${WIDTH}x${HEIGHT}+${CURRENT_X}+${CURRENT_Y}"
            CURRENT_Y=$(( CURRENT_Y + HEIGHT + PADDING ))
        else
            GEOMETRY="+${CURRENT_X}+${CURRENT_Y}"
            CURRENT_Y=$(( CURRENT_Y + 300 + PADDING )) # fallback
        fi
    else
        echo "ℹ️ Offset format: +X+Y (e.g., +0+0 or +1920+0)"
        read -p "$SOCKET_NAME > Enter Custom Offset (+X+Y): " CUSTOM_OFFSET
        if [[ -n "$ASPECT" && -n "$WIDTH" ]]; then
            IFS=":" read -r W H <<< "$ASPECT"
            HEIGHT=$(printf "%.0f" "$(echo "$WIDTH * $H / $W" | bc -l)")
            GEOMETRY="${WIDTH}x${HEIGHT}${CUSTOM_OFFSET}"
        else
            GEOMETRY="${CUSTOM_OFFSET}"
        fi
    fi

    OUTFILE="$INSTALL_DIR/$SOCKET_NAME"
    cp "$SKEL" "$OUTFILE"

    # Patch script with geometry and socket name
    SED_INPLACE "s|MPVC_SOCKET=\"[^\"]*\"|MPVC_SOCKET=\"\${MPVC_SOCKET:-\$HOME/.config/panebot/$SOCKET_NAME}\"|" "$OUTFILE"
    SED_INPLACE "s|\(--geometry=\)[^ ]*|\1$GEOMETRY|" "$OUTFILE"
    chmod +x "$OUTFILE"

    if [[ "$IS_REMOTE" == "y" ]]; then
        echo "📦 Copying to $REMOTE_HOST..."
        scp "$OUTFILE" "$REMOTE_HOST:$REMOTE_INSTALL_DIR/"
    fi

    SCRIPT_NAMES+=("$SOCKET_NAME")

    # Log to pane map
    echo "$SOCKET_NAME|$IS_REMOTE|${REMOTE_HOST:-localhost}|${REMOTE_INSTALL_DIR:-$INSTALL_DIR}" >> "$PANE_MAP"
    echo "✅ Pane '$SOCKET_NAME' ready."
done

echo ""
read -p "🚀 Launch all panes now? (Y/n): " LAUNCH_NOW
LAUNCH_NOW=${LAUNCH_NOW:-Y}
LAUNCH_NOW=$(echo "$LAUNCH_NOW" | tr '[:upper:]' '[:lower:]')

if [[ "$LAUNCH_NOW" == "y" ]]; then
    echo "Launching panes..."
    while IFS="|" read -r NAME IS_REMOTE HOST PATH; do
        if [[ "$IS_REMOTE" == "y" ]]; then
            ssh "$HOST" "$PATH/$NAME --mpv &"
        else
            "$PATH/$NAME" --mpv &
        fi
    done < "$PANE_MAP"
    echo "✅ All panes launched."
else
    echo "✨ Done. Scripts ready in $INSTALL_DIR and pane map in $PANE_MAP"
fi

