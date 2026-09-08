// Trouvé par l'audit du 7 septembre 2026 : la pastille de version « v0.9.2 »
// (id="app-version") portait le clic de vérification des mises à jour (maj.ts)
// mais était un <span> sans role ni tabindex, avec seulement `cursor: pointer`.
// La tabulation la sautait (comme le toggle de thème voisin, déjà passé en
// <button>) et un lecteur d'écran ne l'annonçait pas comme action : la seule
// voie vers checkUpdate() était la souris (échec WCAG 2.1.1 « Clavier » et
// 4.1.2 « Nom, rôle, valeur »). Ces tests lisent le balisage et le CSS de
// index.html et verrouillent que l'élément est désormais un <button> nommé,
// dont l'apparence de pastille est reposée par-dessus le style bouton natif.
//
// On analyse le texte brut de index.html (comme focus-interrupteur-segmente) :
// le CSSOM de jsdom (rrweb-cssom) déforme certains sélecteurs, et on veut de
// toute façon geler le balisage source, pas un DOM reconstruit.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";

/** La balise ouvrante de l'élément qui porte id="app-version". */
function baliseVersion(): string {
  const m = indexHtml.match(/<([a-z]+)\b[^>]*\bid="app-version"[^>]*>/i);
  if (!m) throw new Error("aucun élément id=\"app-version\" dans index.html");
  return m[0];
}

/** Le texte de la feuille de style inline de index.html. */
function css(): string {
  const bloc = indexHtml.match(/<style>([\s\S]*?)<\/style>/);
  if (!bloc) throw new Error("aucun bloc <style> dans index.html");
  return bloc[1];
}

describe("La vérification des mises à jour est atteignable au clavier", () => {
  const balise = baliseVersion();

  it("app-version est un <button>, pas un <span> muet", () => {
    // Un <button> est focalisable au clavier (Tab s'y arrête) et porte le rôle
    // implicite « button » ; un <span> cliquable n'offrait ni l'un ni l'autre.
    expect(balise.startsWith("<button")).toBe(true);
    expect(balise).toMatch(/\btype="button"/);
  });

  it("le bouton annonce son action à un lecteur d'écran", () => {
    // Sans nom accessible, le lecteur d'écran n'aurait lu que « v0.9.2 » sans
    // dire que c'est le déclencheur de la vérification des mises à jour.
    expect(balise).toMatch(/\baria-label="[^"]+"/);
    expect(balise).toMatch(/\bdata-i18n-aria="verifier-les-mises-a-jour"/);
  });

  it("le style de pastille est reposé par-dessus le bouton natif", () => {
    // font/border neutralisent la police système et le liseré du <button> pour
    // que la pastille garde son apparence ; sans quoi convertir le span aurait
    // changé le rendu de la barre latérale.
    const regle = css().match(/\.brand\s+\.ver\s*\{([^}]*)\}/);
    expect(regle, ".brand .ver introuvable").not.toBeNull();
    expect(regle![1]).toMatch(/font:\s*inherit/);
    expect(regle![1]).toMatch(/border:\s*none/);
  });
});
