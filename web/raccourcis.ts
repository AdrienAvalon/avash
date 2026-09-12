// Raccourcis d'onglets et d'accueil.

import { $, state } from "./etat";
import { closeRdp, focusRdp, rdpSessions } from "./rdp";
import { closeSession, focusSession } from "./main";
import { keysOpen } from "./cles";
import { manualOpen, manualSyncSaveRow } from "./connexion-directe";
import { basculerPartage } from "./vue-partagee";
import { askConfirm } from "./dialogues";
import { libelleSur } from "./filters";
import { t } from "./i18n";
import { confirmerFermetureOnglet } from "./prefs";

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
/** Ferme un onglet (croix, Ctrl+W). Un onglet vivant, session SSH ou série
 *  ouverte ou bureau connecté, demande d'abord confirmation ; fermé ou en
 *  connexion, il se ferme sans question.
 *
 *  Audit du 12 septembre 2026 (C-front-6) : toutes les actions destructrices
 *  passaient par une confirmation, sauf la fermeture d'une session vivante,
 *  irréversible pour ce qui tourne dedans. Une croix visée un peu trop à
 *  droite, ou Ctrl+W par réflexe readline, tuait une migration en cours.
 *  « Ne plus demander » se règle à la palette (`prefs.ts`). */
export async function fermerOnglet(o: { kind: "ssh" | "rdp"; id: number }): Promise<void> {
  const ssh = o.kind === "ssh" ? state.sessions.get(o.id) : undefined;
  const rdp = o.kind === "rdp" ? rdpSessions.get(o.id) : undefined;
  const vivant = ssh ? ssh.etat === "live" && !ssh.closed : rdp?.etat === "live";
  if (vivant && confirmerFermetureOnglet()) {
    const nom = ssh?.alias ?? rdp?.tab.querySelector(".label")?.textContent ?? "";
    const ok = await askConfirm(t("onglet-fermer-question", { nom: libelleSur(nom) }), { ok: t("fermer-l-onglet-maj") });
    if (!ok) return;
  }
  if (o.kind === "ssh") closeSession(o.id);
  else closeRdp(o.id);
}
function closeActiveTab() {
  if (state.active === null) return;
  void fermerOnglet({ kind: rdpSessions.has(state.active) ? "rdp" : "ssh", id: state.active });
}

// Ctrl+Maj+E : l'onglet suivant côte à côte, ou la vue partagée refermée (le
// raccourci de Terminator). En phase de capture et arrêté là : quand un
// terminal a le focus, xterm voit la touche avant les écouteurs de la fenêtre
// et la remontait ensuite, si bien que le partage s'ouvrait puis se refermait
// dans la même frappe. Ici, une seule fois, avant tout le monde.
window.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey) return;
  if (e.code !== "KeyE" && e.key.toLowerCase() !== "e") return;
  if (document.querySelector(".modal-backdrop.open, .palette-backdrop.open")) return;
  e.preventDefault();
  e.stopPropagation();
  basculerPartage();
}, true);

window.addEventListener("keydown", (e) => {
  // Les modificateurs d'abord : le balayage du document qui suit coûtait à
  // chaque frappe (88 ms par touche mesurés sous jsdom avec 10 000 entrées
  // SFTP, relevé par la voie FRONT-B de l'audit du 12 septembre 2026).
  const mod = e.ctrlKey || e.metaKey;
  if (!mod) return;
  // Ne pas capturer pendant qu'un formulaire OU la palette est ouvert :
  // l'utilisateur y tape, Ctrl+W fermerait un onglet sous ses doigts.
  if (document.querySelector(".modal-backdrop.open, .palette-backdrop.open")) return;

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
