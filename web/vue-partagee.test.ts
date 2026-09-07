// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : `surFermeture` remettait `volets` à
// null sans réappliquer la vue. Les appelants (`closeRdp`, `closeSession`) ne
// rappellent `appliquerVue` que si l'onglet fermé était l'actif ; fermer le
// volet INACTIF laissait donc `#terminal` avec la classe `partage` et ses deux
// `.volet`, l'actif coincé à moitié de largeur et le volet voisin vide, jusqu'au
// prochain changement d'onglet. Ce test verrouille que fermer un volet inactif
// met bien fin au partage dans le DOM.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// On coupe la vue de son import lourd `./rdp` (Tauri, audio, WebSocket) : cette
// vue ne connaît pas le protocole et un scénario SSH suffit à voir le défaut.
// `rdpSessions` reste vide, `marquerVisibilite` n'est donc jamais appelé.
vi.mock("./rdp", () => ({ rdpSessions: new Map(), marquerVisibilite: vi.fn() }));
// `orderedTabs` sert à `basculerPartage` pour choisir l'onglet du volet droit.
vi.mock("./raccourcis", () => ({
  orderedTabs: () => [
    { kind: "ssh", id: 1 },
    { kind: "ssh", id: 2 },
  ],
}));

import { state, type Session } from "./etat";
import { appliquerVue, basculerPartage, surFermeture, vuePartagee } from "./vue-partagee";

/** Monte une fausse session SSH : un `.xterm-container` dans `#terminal`, dont
 *  l'élément intérieur joue le rôle de `term.element` (ce que la vue déplace). */
function monterSession(id: number): void {
  const conteneur = document.createElement("div");
  conteneur.className = "xterm-container";
  const element = document.createElement("div");
  conteneur.appendChild(element);
  document.getElementById("terminal")!.appendChild(conteneur);
  state.sessions.set(id, { term: { element }, fit: { fit: () => {} } } as unknown as Session);
}

beforeEach(() => {
  document.body.innerHTML = `<div id="terminal"></div><div id="terminal-empty"></div>`;
  state.sessions.clear();
  state.active = 1;
  monterSession(1);
  monterSession(2);
  // jsdom n'exécute pas de frame : la boucle de réajustement de `appliquerVue`
  // n'a pas besoin d'attendre, on la déroule tout de suite.
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    cb(0);
    return 0;
  });
});

afterEach(() => {
  // La vue garde `volets` en variable de module : on la remet à null d'un
  // dernier `surFermeture` pour ne pas fuir l'état d'un test à l'autre.
  if (vuePartagee()) surFermeture({ kind: "ssh", id: state.active ?? 1 });
  vi.unstubAllGlobals();
});

describe("Vue partagée : fin du partage", () => {
  it("fermer_le_volet_inactif_met_fin_au_partage", () => {
    basculerPartage(); // 1 (actif, gauche) | 2 (droit)
    expect(vuePartagee()).toBe(true);
    const terminal = document.getElementById("terminal")!;
    expect(terminal.classList.contains("partage")).toBe(true);
    expect(terminal.querySelectorAll(".volet").length).toBe(2);

    // On ferme l'onglet du volet DROIT, qui n'est pas l'actif : c'est le trou.
    surFermeture({ kind: "ssh", id: 2 });

    expect(vuePartagee()).toBe(false);
    expect(terminal.classList.contains("partage")).toBe(false);
    expect(terminal.querySelectorAll(".volet").length).toBe(0);
    // Le conteneur de l'actif est revenu à la racine de #terminal, visible.
    const actif = state.sessions.get(1)!.term.element!.parentElement as HTMLElement;
    expect(actif.parentElement).toBe(terminal);
    expect(actif.style.display).toBe("block");
  });

  it("fermer_le_volet_actif_met_fin_au_partage", () => {
    // Cas déjà couvert par les appelants (via focusTab), mais on vérifie que
    // `surFermeture` seul nettoie aussi quand l'actif tient un volet.
    basculerPartage(); // 1 (actif, gauche) | 2 (droit)
    surFermeture({ kind: "ssh", id: 1 });

    expect(vuePartagee()).toBe(false);
    const terminal = document.getElementById("terminal")!;
    expect(terminal.classList.contains("partage")).toBe(false);
    expect(terminal.querySelectorAll(".volet").length).toBe(0);
  });

  it("surFermeture_sans_partage_ne_touche_pas_le_DOM", () => {
    // Hors partage, fermer un onglet ne doit rien réappliquer : les appelants
    // s'en chargent selon l'onglet actif.
    const terminal = document.getElementById("terminal")!;
    terminal.classList.add("temoin");
    // Aucun partage en cours : le garde interdit tout effet.
    surFermeture({ kind: "ssh", id: 2 });
    expect(terminal.classList.contains("temoin")).toBe(true);
    // appliquerVue reste appelable directement, sans lever.
    expect(() => appliquerVue()).not.toThrow();
  });
});
