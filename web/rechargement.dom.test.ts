// @vitest-environment jsdom
// Audit du 27 novembre 2026 (WI-1, FI-2, ME-3) : sous WebView2, F5 ou Ctrl+R
// hors d'un terminal rechargeait toute l'interface, et le menu contextuel par
// défaut proposait « Recharger » partout où Avash n'a pas de menu à lui. Les
// onglets disparaissaient, les sessions restaient ouvertes dans le cœur.
import { describe, it, expect, beforeEach } from "vitest";
import { estToucheDeRechargement, installerGardesRechargement, menuNatifPermis } from "./rechargement";

function touche(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

describe("les touches de rechargement", () => {
  it("F5, Ctrl+F5, Maj+F5, Ctrl+R, Ctrl+Maj+R et Cmd+R sont reconnues", () => {
    expect(estToucheDeRechargement(touche({ key: "F5", code: "F5" }))).toBe(true);
    expect(estToucheDeRechargement(touche({ key: "F5", code: "F5", ctrlKey: true }))).toBe(true);
    expect(estToucheDeRechargement(touche({ key: "F5", code: "F5", shiftKey: true }))).toBe(true);
    expect(estToucheDeRechargement(touche({ key: "r", code: "KeyR", ctrlKey: true }))).toBe(true);
    expect(estToucheDeRechargement(touche({ key: "R", code: "KeyR", ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(estToucheDeRechargement(touche({ key: "r", code: "KeyR", metaKey: true }))).toBe(true);
  });

  it("R seul, Ctrl+Alt+R (AltGr) et F4 ne le sont pas", () => {
    expect(estToucheDeRechargement(touche({ key: "r", code: "KeyR" }))).toBe(false);
    expect(estToucheDeRechargement(touche({ key: "r", code: "KeyR", ctrlKey: true, altKey: true }))).toBe(false);
    expect(estToucheDeRechargement(touche({ key: "F4", code: "F4" }))).toBe(false);
  });
});

describe("les gardes posées sur la fenêtre", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <aside id="barre"><div class="ligne">web-1</div></aside>
      <input id="champ">
      <div id="terminal"><textarea class="xterm-helper-textarea"></textarea></div>`;
  });

  it("F5 hors du terminal est annulée", () => {
    const cible = new EventTarget() as unknown as Window;
    const ecouteurs: [string, EventListener, unknown][] = [];
    (cible as unknown as { addEventListener: unknown }).addEventListener = (t: string, f: EventListener, o: unknown) => ecouteurs.push([t, f, o]);
    installerGardesRechargement(cible);
    const [type, ecouteur, capture] = ecouteurs[0];
    expect(type).toBe("keydown");
    expect(capture).toBe(true); // avant xterm et les champs
    const e = touche({ key: "F5", code: "F5" });
    ecouteur(e);
    expect(e.defaultPrevented).toBe(true);
  });

  it("Ctrl+R annulée par le navigateur atteint encore le terminal (recherche inverse de bash)", () => {
    installerGardesRechargement(window);
    const zone = document.querySelector(".xterm-helper-textarea")!;
    let vuParLeTerminal = false;
    zone.addEventListener("keydown", () => (vuParLeTerminal = true));
    const e = touche({ key: "r", code: "KeyR", ctrlKey: true });
    zone.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    expect(vuParLeTerminal).toBe(true);
  });

  it("le menu natif (et son « Recharger ») ne s'ouvre plus hors des champs de saisie", () => {
    installerGardesRechargement(window);
    const surLaBarre = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.querySelector(".ligne")!.dispatchEvent(surLaBarre);
    expect(surLaBarre.defaultPrevented).toBe(true);

    const dansUnChamp = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.getElementById("champ")!.dispatchEvent(dansUnChamp);
    expect(dansUnChamp.defaultPrevented).toBe(false); // couper, copier, coller
  });

  it("le champ caché de xterm n'est pas un champ de saisie ordinaire", () => {
    expect(menuNatifPermis(document.querySelector(".xterm-helper-textarea"))).toBe(false);
    expect(menuNatifPermis(document.getElementById("champ"))).toBe(true);
    expect(menuNatifPermis(null)).toBe(false);
  });
});
