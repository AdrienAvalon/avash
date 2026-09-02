// Panneaux redimensionnables (barre latérale et SFTP).

import { $, state } from "./etat";

// ---------- Panneaux redimensionnables (barre latérale + SFTP) ----------

/** Ajuste le terminal actif à la nouvelle taille (le canvas RDP suit en CSS). */
function fitActive() {
  if (state.active === null) return;
  state.sessions.get(state.active)?.fit.fit();
}

const root = document.documentElement;
function loadPanelPrefs() {
  try {
    const sw = localStorage.getItem("avash.side.w");
    if (sw) root.style.setProperty("--side-w", `${clampSide(Number(sw))}px`);
    const fw = localStorage.getItem("avash.sftp.w");
    if (fw) root.style.setProperty("--sftp-w", `${clampSftp(Number(fw))}px`);
    if (localStorage.getItem("avash.side.collapsed") === "1") document.body.classList.add("side-collapsed");
  } catch { /* stockage indispo */ }
}
const clampSide = (n: number) => Math.max(180, Math.min(460, n));
const clampSftp = (n: number) => Math.max(220, Math.min(640, n));

/**
 * Rend une poignée glissable. `compute(dx, startW)` donne la nouvelle largeur ;
 * le rendu est throttlé au rAF et ré-ajuste le terminal actif à chaque frame.
 */
function attachResizer(
  handle: HTMLElement,
  target: HTMLElement,
  cssVar: string,
  storeKey: string,
  clamp: (n: number) => number,
  compute: (dx: number, startW: number) => number,
) {
  handle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = target.getBoundingClientRect().width;
    let pending = startW;
    let raf = 0;
    document.body.classList.add("resizing");
    const onMove = (ev: MouseEvent) => {
      pending = clamp(compute(ev.clientX - startX, startW));
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        root.style.setProperty(cssVar, `${pending}px`);
        fitActive();
      });
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      if (raf) cancelAnimationFrame(raf);
      document.body.classList.remove("resizing");
      root.style.setProperty(cssVar, `${pending}px`);
      try { localStorage.setItem(storeKey, String(Math.round(pending))); } catch { /* */ }
      fitActive();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  // Double-clic : réinitialise à la largeur par défaut.
  handle.addEventListener("dblclick", () => {
    root.style.removeProperty(cssVar);
    try { localStorage.removeItem(storeKey); } catch { /* */ }
    fitActive();
  });
}

function initPanels() {
  loadPanelPrefs();
  // Barre latérale : glisser le bord droit élargit.
  attachResizer($("side-resize"), document.querySelector(".sidebar") as HTMLElement,
    "--side-w", "avash.side.w", clampSide, (dx, w) => w + dx);
  // Panneau SFTP : bord GAUCHE, glisser vers la gauche élargit.
  attachResizer($("sftp-resize"), $("sftp-panel"), "--sftp-w", "avash.sftp.w", clampSftp, (dx, w) => w - dx);

  const toggleSidebar = () => {
    const collapsed = document.body.classList.toggle("side-collapsed");
    try { localStorage.setItem("avash.side.collapsed", collapsed ? "1" : "0"); } catch { /* */ }
    // Le repli change la largeur sans event resize : on ajuste à la fin.
    setTimeout(fitActive, 0);
  };
  $("side-toggle").addEventListener("click", toggleSidebar);
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === ".") { e.preventDefault(); toggleSidebar(); }
  });
}
initPanels();
