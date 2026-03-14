#!/bin/bash

echo "👋 Welcome to Marlovious."
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
            CURRENT_Y=$(( CURRENT_Y + 300 + PADDING )) # fallback estimate
        fi
    else
        echo "ℹ️  Offset format: +X+Y (e.g., +0+0 for top-left, +1920+0 for second monitor)"
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

    # Replace values in the script
    SED_INPLACE "s|MPVC_SOCKET=\"[^\"]*\"|MPVC_SOCKET=\"\${MPVC_SOCKET:-\$MPVC_CONFIG_DIR/$SOCKET_NAME}\"|" "$OUTFILE"
    SED_INPLACE "s|\(--geometry=\)[^ ]*|\1$GEOMETRY|" "$OUTFILE"

    chmod +x "$OUTFILE"
    SCRIPT_NAMES+=("$SOCKET_NAME")
    echo "✅ Created: $OUTFILE"
done

echo ""
read -p "🚀 Launch all panes now? (Y/n): " LAUNCH_NOW
LAUNCH_NOW=${LAUNCH_NOW:-Y}

if [[ "$LAUNCH_NOW" =~ ^[Yy]$ ]]; then
    echo "Launching panes..."
    for SCRIPT in "${SCRIPT_NAMES[@]}"; do
        "$INSTALL_DIR/$SCRIPT" --mpv &
    done
    echo "✅ All panes launched."
else
    echo "✨ Done. You can launch them later from $INSTALL_DIR"
fi

