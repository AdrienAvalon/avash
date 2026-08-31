#!/usr/bin/env python3
"""Mesure le trafic de trames RDP en se branchant comme le fait l'interface.

Pourquoi cet outil : le processus RDP n'accumulait qu'une **union englobante**
des zones modifiées. Deux poussières aux coins opposés donnaient un rectangle
plein écran. Personne ne pouvait le voir — ni les tests, qui ne regardaient pas
le fil, ni l'utilisateur, pour qui c'était seulement « un peu lent ».

Mesuré contre un vrai xrdp, même parcours de souris, 20 secondes :

    union englobante   6 trames,  6 rectangles, 8,39 Mo
    fusion sélective   9 trames, 20 rectangles, 4,36 Mo

Moitié moins d'octets, et davantage de trames livrées.

Sortie : « trames rectangles octets ». Code 1 si rien n'a été reçu.
Dépend de python-websockets.

Usage : mesure-trames.py <port RDP> <secondes>
"""
import asyncio
import subprocess
import sys
import time

import websockets

COINS = [(60, 60), (1200, 60), (1200, 740), (60, 740), (640, 400)]


async def mesurer(port, token, secondes):
    total = trames = rects = 0
    async with websockets.connect(f"ws://127.0.0.1:{port}", max_size=None) as ws:
        await ws.send(token.encode())  # binaire, comme l'interface
        fin = time.monotonic() + secondes
        i, prochain = 0, time.monotonic()
        while time.monotonic() < fin:
            if time.monotonic() >= prochain:
                # Animer la session : sans mouvement, un bureau au repos
                # n'envoie presque rien et la mesure ne dit rien.
                x, y = COINS[i % len(COINS)]
                i += 1
                prochain = time.monotonic() + 0.4
                await ws.send(bytes([1]) + x.to_bytes(2, "little") + y.to_bytes(2, "little"))
            try:
                m = await asyncio.wait_for(ws.recv(), timeout=0.3)
            except (asyncio.TimeoutError, websockets.exceptions.ConnectionClosed):
                continue
            if not isinstance(m, (bytes, bytearray)) or not m:
                continue
            if m[0] == 2:      # trame à un seul rectangle
                total, trames, rects = total + len(m), trames + 1, rects + 1
                await ws.send(bytes([6]))
            elif m[0] == 13:   # trame à plusieurs rectangles
                total, trames, rects = total + len(m), trames + 1, rects + m[1]
                await ws.send(bytes([6]))
    return total, trames, rects


async def main():
    port_rdp, secondes = sys.argv[1], float(sys.argv[2])
    p = subprocess.Popen(
        ["rdp-sidecar/target/release/avash-rdp", "--host", "127.0.0.1", "--port", port_rdp,
         "-u", "essai", "-p", "essai-mot-de-passe", "--sans-nla",
         "--width", "1280", "--height", "800"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    annonce = (p.stdout.readline() or "").split()
    if len(annonce) != 2:
        print("  le processus n'a annoncé aucun point de connexion", file=sys.stderr)
        p.kill()
        return 1
    port, token = annonce
    try:
        total, trames, rects = await mesurer(int(port), token, secondes)
    finally:
        p.kill()
    if trames == 0:
        print("  aucune trame reçue", file=sys.stderr)
        return 1
    print(f"{trames} {rects} {total}")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
