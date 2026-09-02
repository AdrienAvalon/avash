// Raccourcis d'onglets et d'accueil.

import { $, state } from "./etat";
import { closeRdp, focusRdp, rdpSessions } from "./rdp";
import { closeSession, focusSession } from "./main";
import { keysOpen } from "./cles";
import { manualOpen, manualSyncSaveRow } from "./connexion-directe";

// ---------- Raccourcis d'onglets ----------

/** Liste ordonnee des identifiants de session, pour naviguer par position. */
/** Tous les onglets (SSH + RDP) dans l'ordre du DOM. */
export function orderedTabs(): { kind: "ssh" | "rdp"; id: number }[] {
  const byEl = new Map<HTMLElement, { kind: "ssh" | "rdp"; id: number }>();
  for (const [id, sess] of state.sessions) byEl.set(sess.tab, { kind: "ssh", id });
  for (const [id, sess] of rdpSessions) byEl.set(sess.tab, { kind: "rdp", id });
  const out: { kind: "ssh" | "rdp"; id: number }[] = [];
  for (const el of $("tabs").querySelectorAll<HTMLElement>(".tab")) {
    const t = byEl.get(el);
    if (t) out.push(t);
  }
  return out;
}
export function focusTab(t: { kind: "ssh" | "rdp"; id: number }) {
  if (t.kind === "ssh") focusSession(t.id);
  else focusRdp(t.id);
}
function closeActiveTab() {
  if (state.active === null) return;
  if (rdpSessions.has(state.active)) closeRdp(state.active);
  else closeSession(state.active);
}

window.addEventListener("keydown", (e) => {
  // Ne pas capturer pendant qu'un formulaire OU la palette est ouvert :
  // l'utilisateur y tape, Ctrl+W fermerait un onglet sous ses doigts.
  if (document.querySelector(".modal-backdrop.open, .palette-backdrop.open")) return;
  const mod = e.ctrlKey || e.metaKey;
  if (!mod) return;

  if (e.key.toLowerCase() === "w" && state.active !== null) {
    e.preventDefault();
    closeActiveTab();
    return;
  }
  const tabs = orderedTabs();
  if (e.key === "Tab") {
    if (tabs.length < 2) return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === state.active);
    const step = e.shiftKey ? -1 : 1;
    focusTab(tabs[(Math.max(0, i) + step + tabs.length) % tabs.length]);
    return;
  }
  // Ctrl+1..9 : acces direct a un onglet (SSH ou RDP) par sa position.
  if (/^[1-9]$/.test(e.key)) {
    const idx = Number(e.key) - 1;
    if (idx < tabs.length) {
      e.preventDefault();
      focusTab(tabs[idx]);
    }
  }
});

// L'ecran d'accueil s'adapte : sans hote declare, le conseil « double-clic
// sur un hote » est inapplicable.
$("empty-connect").addEventListener("click", manualOpen);
$("empty-keys").addEventListener("click", keysOpen);
manualSyncSaveRow();
