// @vitest-environment jsdom
// Le relais des <style> de xterm.js vers des feuilles adoptées (styles-xterm.ts).
// Régression du 27 novembre 2026 : sous la CSP d'Avash (nonce ajouté par
// Tauri), ces éléments sont refusés, et le rendu DOM de Linux perdait sa police
// monospace depuis la 0.14.2.
import { describe, it, expect, beforeEach } from "vitest";
import { refleterStylesXterm, cesserStylesXterm } from "./styles-xterm";

const tick = () => new Promise((r) => setTimeout(r, 0));
const regles = () => document.adoptedStyleSheets.flatMap((f) => [...f.cssRules].map((r) => r.cssText));

function terminalFactice(): { racine: HTMLElement; ecran: HTMLElement; style: HTMLStyleElement } {
  const racine = document.createElement("div");
  racine.className = "terminal xterm";
  const ecran = document.createElement("div");
  ecran.className = "xterm-screen";
  const style = document.createElement("style");
  style.textContent = '.xterm-dom-renderer-owner-1 .xterm-rows { font-family: "Avash Mono", monospace; font-size: 14px; }';
  ecran.appendChild(style);
  const lignes = document.createElement("div");
  lignes.className = "xterm-rows";
  ecran.appendChild(lignes);
  racine.appendChild(ecran);
  document.body.appendChild(racine);
  return { racine, ecran, style };
}

describe("styles de xterm.js sous la CSP", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    document.adoptedStyleSheets = [];
  });

  it("le CSS d'un <style> de xterm devient une feuille adoptée", () => {
    const { racine } = terminalFactice();
    refleterStylesXterm(racine);
    expect(regles().join("\n")).toMatch(/xterm-rows[^}]*font-family: "?Avash Mono"?, monospace/);
  });

  it("la feuille suit le texte que xterm réécrit (taille, thème)", async () => {
    const { racine, style } = terminalFactice();
    refleterStylesXterm(racine);
    style.textContent = ".xterm-dom-renderer-owner-1 .xterm-rows { font-size: 18px; }";
    await tick();
    expect(regles().join("\n")).toContain("font-size: 18px");
    expect(regles().join("\n")).not.toContain("14px");
  });

  it("un <style> ajouté plus tard dans l'écran est repris (rendu DOM après perte du contexte WebGL)", async () => {
    const { racine, ecran } = terminalFactice();
    refleterStylesXterm(racine);
    const tardif = document.createElement("style");
    tardif.textContent = ".xterm .xterm-scrollable-element > .scrollbar > .slider { background: red; }";
    ecran.appendChild(tardif);
    await tick();
    expect(regles().join("\n")).toContain(".slider");
  });

  it("à la fermeture, ses feuilles partent et celles des autres restent", () => {
    const autre = new CSSStyleSheet();
    autre.replaceSync(".a-nous { color: blue; }");
    document.adoptedStyleSheets = [autre];
    const t1 = terminalFactice();
    const t2 = terminalFactice();
    t2.style.textContent = ".xterm-dom-renderer-owner-2 .xterm-rows { font-size: 16px; }";
    refleterStylesXterm(t1.racine);
    refleterStylesXterm(t2.racine);
    cesserStylesXterm(t1.racine);
    const texte = regles().join("\n");
    expect(texte).toContain(".a-nous");
    expect(texte).toContain("owner-2");
    expect(texte).not.toContain("owner-1");
  });

  it("les lignes du terminal qui changent ne déclenchent aucune relecture", async () => {
    const { racine, ecran } = terminalFactice();
    refleterStylesXterm(racine);
    const avant = document.adoptedStyleSheets;
    const lignes = ecran.querySelector(".xterm-rows")!;
    for (let i = 0; i < 50; i++) lignes.appendChild(document.createElement("span"));
    await tick();
    // Même tableau : rien n'a été réadopté pour des lignes de texte.
    expect(document.adoptedStyleSheets).toBe(avant);
  });
});
