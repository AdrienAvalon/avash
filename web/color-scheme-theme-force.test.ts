// Trouvé par l'audit du 7 septembre 2026 : `html { color-scheme: light dark }`
// laisse le navigateur choisir le rendu des contrôles natifs (popups des
// <select> : protocole, vitesse série, hôte de tunnel, clé à installer ;
// <datalist> des ports série ; bouton de révélation du mot de passe ; fond par
// défaut des contrôles) d'après prefers-color-scheme, sans suivre le thème
// forcé. Système clair + Avash forcé en sombre : la liste déroulante s'ouvrait
// en blanc sur une modale sombre (et l'inverse en noir sur blanc) sur WebView2
// (Windows) et WKWebView (macOS), qui peignent ces popups eux-mêmes. Le
// correctif fige color-scheme dans les blocs data-theme. Ce test lit le CSS de
// index.html et verrouille les trois règles.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";

/** Le texte de la feuille de style inline de index.html. */
function css(): string {
  const bloc = indexHtml.match(/<style>([\s\S]*?)<\/style>/);
  if (!bloc) throw new Error("aucun bloc <style> dans index.html");
  return bloc[1];
}

/** Le corps `{ ... }` (déclarations, sans accolade imbriquée) d'un sélecteur. */
function corpsRegle(source: string, selecteurEchappe: string): string {
  const m = source.match(new RegExp(selecteurEchappe + "\\s*\\{([^}]*)\\}"));
  if (!m) throw new Error(`règle introuvable : ${selecteurEchappe}`);
  return m[1];
}

const source = css();

describe("color-scheme suit le thème forcé", () => {
  it("html garde `color-scheme: light dark` pour le mode système", () => {
    // Sans data-theme (préférence « système »), on laisse le navigateur
    // choisir clair/sombre d'après prefers-color-scheme : les contrôles natifs
    // suivent alors le système, ce qui est correct puisque l'interface aussi.
    const bloc = corpsRegle(source, "html");
    expect(bloc).toMatch(/color-scheme:\s*light dark\b/);
  });

  it("data-theme=\"light\" force les contrôles natifs en clair", () => {
    // Système sombre, Avash forcé en clair : sans cette règle color-scheme
    // restait `light dark` et le moteur peignait la liste déroulante en noir
    // sur la modale claire.
    const bloc = corpsRegle(source, ':root\\[data-theme="light"\\]');
    expect(bloc).toMatch(/color-scheme:\s*light\s*;/);
  });

  it("data-theme=\"dark\" force les contrôles natifs en sombre", () => {
    // Système clair, Avash forcé en sombre : le cas du scénario d'audit, la
    // liste s'ouvrait en blanc sur une modale sombre.
    const bloc = corpsRegle(source, ':root\\[data-theme="dark"\\]');
    expect(bloc).toMatch(/color-scheme:\s*dark\s*;/);
  });
});
