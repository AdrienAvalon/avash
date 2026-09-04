// Vue partagée : deux onglets côte à côte dans la zone centrale, un par volet.
//
// La zone centrale (`#terminal`) tient tous les conteneurs de session, un seul
// affiché à la fois. En vue partagée, deux volets la coupent en deux et
// reçoivent chacun le conteneur d'un onglet ; les autres restent cachés à la
// racine. L'onglet actif (`state.active`) est celui des deux qui a le clavier :
// cliquer un onglet qui n'est dans aucun volet le met à la place de l'actif,
// fermer l'un des deux met fin au partage. Rien ici ne connaît le protocole :
// un terminal SSH et un bureau RDP se partagent l'écran de la même façon.

import { $, state } from "./etat";
import { marquerVisibilite, rdpSessions } from "./rdp";
import { orderedTabs } from "./raccourcis";

export type Onglet = { kind: "ssh" | "rdp"; id: number };

let volets: { gauche: Onglet; droit: Onglet } | null = null;

const meme = (a: Onglet | null | undefined, b: Onglet | null | undefined): boolean =>
  !!a && !!b && a.kind === b.kind && a.id === b.id;

export function vuePartagee(): boolean {
  return volets !== null;
}

/** L'onglet actif, sous la forme que la vue manipule. */
function actif(): Onglet | null {
  if (state.active === null) return null;
  return rdpSessions.has(state.active) ? { kind: "rdp", id: state.active } : { kind: "ssh", id: state.active };
}

/** Les onglets affichés : les deux volets, ou le seul actif. */
export function ongletsAffiches(): Onglet[] {
  if (volets) return [volets.gauche, volets.droit];
  const a = actif();
  return a ? [a] : [];
}

export function estAffiche(o: Onglet): boolean {
  return ongletsAffiches().some((x) => meme(x, o));
}

function conteneur(o: Onglet): HTMLElement | null {
  if (o.kind === "ssh") return (state.sessions.get(o.id)?.term.element?.parentElement as HTMLElement | null) ?? null;
  return (rdpSessions.get(o.id)?.canvas.parentElement as HTMLElement | null) ?? null;
}

/** Partage l'écran : l'actif à gauche, `droit` à droite. Sans effet si `droit` est l'actif. */
function partager(droit: Onglet): void {
  const a = actif();
  if (!a || meme(a, droit)) return;
  volets = { gauche: a, droit };
  appliquerVue();
}

function fermerPartage(): void {
  if (!volets) return;
  volets = null;
  appliquerVue();
}

/** Ctrl+\ et la palette : partager avec l'onglet suivant, ou refermer. */
export function basculerPartage(): void {
  if (volets) {
    fermerPartage();
    return;
  }
  const tabs = orderedTabs();
  if (tabs.length < 2 || state.active === null) return;
  const i = tabs.findIndex((t) => t.id === state.active);
  partager(tabs[(Math.max(0, i) + 1) % tabs.length]);
}

/** Un onglet prend le focus : s'il n'est dans aucun volet, il remplace l'actif. */
export function surFocus(o: Onglet, precedent: Onglet | null): void {
  if (!volets || estAffiche(o)) return;
  if (meme(volets.gauche, precedent)) volets = { ...volets, gauche: o };
  else if (meme(volets.droit, precedent)) volets = { ...volets, droit: o };
  else volets = { ...volets, gauche: o };
}

/** Un onglet se ferme : s'il tenait un volet, le partage s'arrête. */
export function surFermeture(o: Onglet): void {
  if (volets && (meme(volets.gauche, o) || meme(volets.droit, o))) volets = null;
}

/**
 * Applique la vue au DOM : volets créés ou retirés, conteneurs déplacés, les
 * affichés montrés, les autres cachés, puis les terminaux réajustés et les
 * bureaux prévenus de leur visibilité (un bureau caché ne reçoit plus d'images).
 */
export function appliquerVue(): void {
  const zone = $("terminal");
  const affiches = ongletsAffiches();
  // Tout revient à la racine avant de replacer : un conteneur ne doit vivre
  // que dans un volet qui existe encore.
  for (const v of Array.from(zone.querySelectorAll<HTMLElement>(":scope > .volet"))) {
    while (v.firstChild) zone.appendChild(v.firstChild);
    v.remove();
  }
  zone.classList.toggle("partage", volets !== null);
  if (volets) {
    for (const [nom, o] of [["gauche", volets.gauche], ["droit", volets.droit]] as const) {
      const volet = document.createElement("div");
      volet.className = `volet ${nom}`;
      const c = conteneur(o);
      if (c) volet.appendChild(c);
      zone.appendChild(volet);
    }
  }
  state.sessions.forEach((s, id) => {
    const visible = affiches.some((x) => x.kind === "ssh" && x.id === id);
    const c = s.term.element?.parentElement as HTMLElement | undefined;
    if (c) c.style.display = visible ? "block" : "none";
  });
  for (const [id, r] of rdpSessions) {
    const visible = affiches.some((x) => x.kind === "rdp" && x.id === id);
    (r.canvas.parentElement as HTMLElement).style.display = visible ? "flex" : "none";
    marquerVisibilite(r, visible);
    if (visible) r.syncSize?.();
  }
  if (affiches.length > 0) $("terminal-empty").style.display = "none";
  requestAnimationFrame(() => {
    for (const o of affiches) if (o.kind === "ssh") state.sessions.get(o.id)?.fit.fit();
  });
}
