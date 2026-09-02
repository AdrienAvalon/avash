// Barre de titre intégrée (fenêtre sans décorations).

import { getCurrentWindow } from "@tauri-apps/api/window";
import { ic } from "./icons";
import { $, state } from "./etat";

// ---------- Barre de titre custom (decorations: false) ----------

export async function setupWindowControls() {
  const win = getCurrentWindow();
  $("win-min").innerHTML = ic("winMin");
  $("win-close").innerHTML = ic("winClose");
  const maxBtn = $("win-max");
  // On ne réécrit le bouton que si l'état a réellement changé : réinjecter le
  // même SVG force une réanalyse HTML pour rien.
  let maxAffiche: boolean | null = null;
  const paintMax = async () => {
    const maximisee = await win.isMaximized();
    if (maximisee === maxAffiche) return;
    maxAffiche = maximisee;
    maxBtn.innerHTML = ic(maximisee ? "winRestore" : "winMax");
  };
  await paintMax();
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
  void win.onResized(() => {
    window.clearTimeout(repeindreMax);
    repeindreMax = window.setTimeout(() => void paintMax(), 150);
  });
  // Fenêtre en arrière-plan : geler les animations (CPU au repos ~0).
  void win.onFocusChanged(({ payload: focused }) => {
    document.body.classList.toggle("win-blur", !focused);
  });

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
}

/** Reflète la session active dans la barre de titre (utile + évite le doublon). */
export function setTitlebar() {
  const s = state.active === null ? null : state.sessions.get(state.active);
  $("tb-name").textContent = s && !s.closed ? `${s.alias} — Avash` : "Avash";
}
