#!/bin/bash

# Welcome to PaneBot
echo "🙋 Welcome to PaneBot!"
read -p "How many video panes would you like to set up? " PANE_COUNT

SKEL="./skel.sh"
[[ ! -f "$SKEL" ]] && echo "❌ Missing skel.sh in current directory." && exit 1

CONFIG_DIR="$HOME/.config/panebot"
mkdir -p "$CONFIG_DIR"
PANE_MAP_FILE="$CONFIG_DIR/pane_map"

# Determine default install dir
if [[ -d "$HOME/.local/bin" ]]; then
    DEFAULT_INSTALL_DIR="$HOME/.local/bin"
else
    DEFAULT_INSTALL_DIR="$HOME/bin"
    mkdir -p "$DEFAULT_INSTALL_DIR"
fi

# sed -i wrapper for GNU/BSD compatibility
if sed --version >/dev/null 2>&1; then
    SED_INPLACE() { sed -i "$@"; }
else
    SED_INPLACE() { sed -i '' "$@"; }
fi

# Offset tracking for vertical stacking
CURRENT_X=0
CURRENT_Y=0
PADDING=10
SCRIPT_NAMES=()

for (( i=1; i<=PANE_COUNT; i++ )); do
    echo ""
    echo "🔧 Configuring Pane #$i"

    read -p "Pane $i > Name this video pane (No Spaces): " SOCKET_NAME

    read -p "$SOCKET_NAME > Is this pane on a remote host? (y/N): " IS_REMOTE
    IS_REMOTE=${IS_REMOTE,,} # to lowercase

    if [[ "$IS_REMOTE" == "y" ]]; then
        read -p "$SOCKET_NAME > Enter remote SSH user@host: " REMOTE_HOST

        read -p "$SOCKET_NAME > Use default install dir (~/.local/bin) on remote? (Y/n): " USE_DEFAULT_REMOTE_PATH
        if [[ "$USE_DEFAULT_REMOTE_PATH" =~ ^[Nn]$ ]]; then
            read -p "$SOCKET_NAME > Enter custom remote install path: " REMOTE_INSTALL_DIR
        else
            REMOTE_INSTALL_DIR="~/.local/bin"
        fi

        OUTFILE="$REMOTE_INSTALL_DIR/$SOCKET_NAME"

        read -p "$SOCKET_NAME > Generate and push SSH key to $REMOTE_HOST? (Y/n): " PUSH_KEY
        if [[ "$PUSH_KEY" =~ ^[Yy]$ ]]; then
            if [[ ! -f "$HOME/.ssh/id_rsa.pub" ]]; then
                echo "🔐 Generating new SSH key..."
                ssh-keygen -t rsa -b 4096 -f "$HOME/.ssh/id_rsa" -N ""
            fi
            echo "🔑 Pushing SSH key to $REMOTE_HOST..."
            ssh-copy-id "$REMOTE_HOST"
        fi

        echo "💾 Uploading customized pane script to remote..."
        TEMP_SCRIPT="/tmp/$SOCKET_NAME"
        cp "$SKEL" "$TEMP_SCRIPT"

        SED_INPLACE "s|MPVC_SOCKET=\"[^\"]*\"|MPVC_SOCKET=\"\${MPVC_SOCKET:-$CONFIG_DIR/$SOCKET_NAME}\"|" "$TEMP_SCRIPT"
        SED_INPLACE "s|\(--geometry=\)[^ ]*|\1+0+0|" "$TEMP_SCRIPT"
        chmod +x "$TEMP_SCRIPT"
        scp "$TEMP_SCRIPT" "$REMOTE_HOST:$REMOTE_INSTALL_DIR/$SOCKET_NAME"
        rm "$TEMP_SCRIPT"

        echo "$SOCKET_NAME|remote|$REMOTE_HOST|$REMOTE_INSTALL_DIR/$SOCKET_NAME" >> "$PANE_MAP_FILE"
        continue
    fi

    # LOCAL PANE
    read -p "$SOCKET_NAME > Specify Aspect Ratio (1:1, 4:3, 16:9, 2.35:1) [ENTER for dynamic]: " ASPECT
    if [[ -n "$ASPECT" ]]; then
        read -p "$SOCKET_NAME > Specify Width: " WIDTH
    fi

    read -p "$SOCKET_NAME > Auto-place this pane based on previous layout? (Y/n): " AUTO_PLACE
    AUTO_PLACE=${AUTO_PLACE:-Y}

    if [[ "$AUTO_PLACE" =~ ^[Yy]$ ]]; then
        if [[ -n "$ASPECT" && -n "$WIDTH" ]]; then
            IFS=":" read -r W H <<< "$ASPECT"
            HEIGHT=$(printf "%.0f" "$(echo "$WIDTH * $H / $W" | bc -l)")
            GEOMETRY="${WIDTH}x${HEIGHT}+${CURRENT_X}+${CURRENT_Y}"
            CURRENT_Y=$(( CURRENT_Y + HEIGHT + PADDING ))
        else
            GEOMETRY="+${CURRENT_X}+${CURRENT_Y}"
            CURRENT_Y=$(( CURRENT_Y + 300 + PADDING ))
        fi
    else
        echo "ℹ️  Offset format: +X+Y (e.g., +0+0 for top-left, +1920+0 for second monitor)"
        read -p "$SOCKET_NAME > Enter Custom Offset (+X+Y): " CUSTOM_OFFSET
        if [[ -n "$ASPECT" && -n "$WIDTH" ]]; then
            IFS=":" read -r W H <<< "$ASPECT"
            HEIGHT=$(printf "%.0f" "$(echo "$WIDTH * $H / $W" | bc -l)")
            GEOMETRY="${WIDTH}x${HEIGHT}${CUSTOM_OFFSET}"
        else
            GEOMETRY="$CUSTOM_OFFSET"
        fi
    fi

    OUTFILE="$DEFAULT_INSTALL_DIR/$SOCKET_NAME"
    cp "$SKEL" "$OUTFILE"

    SED_INPLACE "s|MPVC_SOCKET=\"[^\"]*\"|MPVC_SOCKET=\"\${MPVC_SOCKET:-$CONFIG_DIR/$SOCKET_NAME}\"|" "$OUTFILE"
    SED_INPLACE "s|\(--geometry=\)[^ ]*|\1$GEOMETRY|" "$OUTFILE"
    chmod +x "$OUTFILE"

    echo "$SOCKET_NAME|local|$(hostname)|$OUTFILE" >> "$PANE_MAP_FILE"
    SCRIPT_NAMES+=("$SOCKET_NAME")
    echo "✅ Created: $OUTFILE"
done

echo ""
read -p "🚀 Launch all local panes now? (Y/n): " LAUNCH_NOW
LAUNCH_NOW=${LAUNCH_NOW:-Y}

if [[ "$LAUNCH_NOW" =~ ^[Yy]$ ]]; then
    echo "Launching panes..."
    for SCRIPT in "${SCRIPT_NAMES[@]}"; do
        "$DEFAULT_INSTALL_DIR/$SCRIPT" --mpv &
    done
    echo "✅ All local panes launched."
else
    echo "✨ Done. You can launch them later from $DEFAULT_INSTALL_DIR"
fi
