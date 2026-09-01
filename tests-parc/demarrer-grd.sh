#!/bin/bash
# Démarre un compositeur sans écran, puis GNOME Remote Desktop par-dessus.
set -e
UID_E="$(id -u essai)"; GID_E="$(id -g essai)"
mkdir -p /run/dbus
dbus-daemon --system --fork 2>/dev/null || true
install -d -m 700 -o "$UID_E" -g "$GID_E" "/run/user/$UID_E"
install -d -m 755 -o "$UID_E" -g "$GID_E" /home/essai/.local/share
install -d -m 755 -o "$UID_E" -g "$GID_E" /home/essai/.local/state
# Xwayland exige ce répertoire inscriptible : sans lui, mutter démarre, crée sa
# sortie virtuelle, puis meurt — et GRD reste un pont vers rien.
install -d -m 1777 /tmp/.X11-unix

exec setpriv --reuid="$UID_E" --regid="$GID_E" --init-groups env \
  "XDG_RUNTIME_DIR=/run/user/$UID_E" HOME=/home/essai \
  XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY=wayland-0 \
  bash -c '
    set -x
    eval "$(dbus-launch --sh-syntax)"
    # PipeWire transporte les images du compositeur vers GRD.
    pipewire & sleep 1
    wireplumber & sleep 1
    # Compositeur sans écran, avec une sortie virtuelle : sans lui, GRD est un
    # pont qui ne mène nulle part.
    mutter --headless --virtual-monitor 1280x800 & sleep 4

    printf "essai-mot-de-passe\n" | grdctl --headless rdp set-credentials essai 2>/dev/null \
      || grdctl --headless rdp set-credentials essai essai-mot-de-passe
    grdctl --headless rdp disable-view-only || true
    grdctl --headless rdp disable-port-negotiation || true

    # Le démon d'abord : posé AVANT lui, le certificat est refusé
    # (« BIO_new failed for certificate »), et le serveur ne se lie jamais.
    /usr/libexec/gnome-remote-desktop-daemon --headless &
    DEMON=$!
    sleep 3
    grdctl --headless rdp set-tls-cert /home/essai/.local/share/gnome-remote-desktop/tls.crt
    grdctl --headless rdp set-tls-key /home/essai/.local/share/gnome-remote-desktop/tls.key
    grdctl --headless rdp enable
    grdctl --headless status || true
    wait "$DEMON"
  '
