// Barre de titre intégrée (fenêtre sans décorations).

import { getCurrentWindow } from "@tauri-apps/api/window";
import { ic } from "./icons";
import { $, state } from "./etat";
import { rdpSessions } from "./rdp";
import { libelleSur } from "./filters";

// ---------- Barre de titre custom (decorations: false) ----------

export async function setupWindowControls() {
  const win = getCurrentWindow();
  $("win-min").innerHTML = ic("winMin");
  $("win-close").innerHTML = ic("winClose");
  const maxBtn = $("win-max");
  // Icône par défaut tout de suite, corrigée dès que la fenêtre répond. On ne
  // réécrit ensuite le bouton que si l'état a réellement changé : réinjecter le
  // même SVG force une réanalyse HTML pour rien.
  maxBtn.innerHTML = ic("winMax");
  let maxAffiche = false;
  const paintMax = async () => {
    let maximisee: boolean;
    try {
      maximisee = await win.isMaximized();
    } catch {
      return; // état illisible : l'icône par défaut reste, le bouton marche
    }
    if (maximisee === maxAffiche) return;
    maxAffiche = maximisee;
    maxBtn.innerHTML = ic(maximisee ? "winRestore" : "winMax");
  };
  // Les boutons se câblent AVANT tout `await` : un `isMaximized()` rejeté
  // laissait réduire, agrandir et fermer sans effet, sans un mot, sur une
  // fenêtre sans décorations (audit du 12 septembre 2026, C-SIL-13).
  $("win-min").addEventListener("click", () => win.minimize());
  maxBtn.addEventListener("click", async () => { await win.toggleMaximize(); await paintMax(); });
  $("win-close").addEventListener("click", () => win.close());
  // L'état maximisé change aussi via double-clic système / raccourci.
  //
  // onResized se déclenche à CHAQUE image pendant qu'on tire la fenêtre. Y
  // enchaîner un aller-retour IPC (isMaximized) saturait le pont avec le
  // backend et figeait l'interface au bout de quelques secondes de glissé —
  // sans même qu'une session soit ouverte. L'état maximisé ne peut changer
  // qu'au terme du geste : on attend que le redimensionnement se pose.
  let repeindreMax: number | undefined;
  win.onResized(() => {
    window.clearTimeout(repeindreMax);
    repeindreMax = window.setTimeout(() => void paintMax(), 150);
  }).catch(() => { /* sans l'événement, l'icône se corrige au prochain clic */ });
  // Fenêtre en arrière-plan : geler les animations (CPU au repos ~0).
  win.onFocusChanged(({ payload: focused }) => {
    document.body.classList.toggle("win-blur", !focused);
  }).catch(() => { /* sans l'événement, les animations tournent : sans gravité */ });

  // Poignées de redimensionnement : sans décorations, la fenêtre n'a plus de
  // bords redimensionnables (surtout sous Wayland). On les recrée nous-mêmes.
  const dirs: [string, string][] = [
    ["rh-n", "North"], ["rh-s", "South"], ["rh-e", "East"], ["rh-w", "West"],
    ["rh-ne", "NorthEast"], ["rh-nw", "NorthWest"], ["rh-se", "SouthEast"], ["rh-sw", "SouthWest"],
  ];
  const box = $("resize-handles");
  for (const [cls, dir] of dirs) {
    const h = document.createElement("div");
    h.className = cls;
    h.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      void win.startResizeDragging(dir as never);
    });
    box.appendChild(h);
  }
  await paintMax();
}

/** Reflète l'onglet actif dans la barre de titre (utile + évite le doublon).
 *
 *  Trouvé par l'audit du 7 septembre 2026 : le titre ne lisait que
 *  `state.sessions`, si bien qu'un onglet RDP actif affichait « Avash » au lieu
 *  du nom du bureau. On nomme désormais aussi un bureau RDP, en reprenant le
 *  libellé déjà posé sur son onglet (source unique, cohérent avec la barre
 *  latérale). `state.active` reste la source du titre — en vue partagée deux
 *  onglets sont affichés mais un seul a le clavier. */
export function setTitlebar() {
  $("tb-name").textContent = nomOngletActif() ?? "Avash";
}

function nomOngletActif(): string | null {
  if (state.active === null) return null;
  // `libelleSur` : sans contrôle de direction, qui réordonnait le titre (audit
  // du 12 septembre 2026, FS-10).
  const s = state.sessions.get(state.active);
  if (s) return s.closed ? null : `${libelleSur(s.alias)} — Avash`;
  const r = rdpSessions.get(state.active);
  // Un bureau fermé ne se nomme plus, comme un onglet SSH fermé : il gardait
  // son nom après une coupure du serveur (même audit, C-SIL-1).
  if (!r || r.etat === "closed") return null;
  const nom = r.tab.querySelector<HTMLElement>(".label")?.textContent;
  return nom ? `${libelleSur(nom)} — Avash` : null;
}
