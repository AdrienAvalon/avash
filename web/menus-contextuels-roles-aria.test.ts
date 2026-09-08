// Trouvé par l'audit du 7 septembre 2026 : ouvrirMenuAuClavier (menu-hote.ts)
// rend les menus contextuels utilisables au clavier après Maj+F10 (flèches,
// Entrée, Échap), mais les conteneurs `.ctx-menu` et leurs entrées `.ctx-item`
// ne portaient aucun rôle ARIA, et les `.ctx-sep` non plus. À Maj+F10 sur un
// hôte, Orca/NVDA lisaient « Se connecter » comme du texte : ni « menu », ni
// « élément de menu », ni position (« 1 sur 6 »), alors que Entrée agit — les
// lignes d'hôte avaient pourtant déjà reçu role="button" (main.ts) pour la
// même raison (échec WCAG 4.1.2 « Nom, rôle, valeur »). Ce test lit le balisage
// brut de index.html et verrouille role="menu"/"menuitem"/"separator" sur les
// cinq menus ; il vérifie aussi que le grisage de « Copier » (terminal-outils)
// porte aria-disabled et pas seulement la classe .disabled invisible au lecteur.
//
// On analyse le texte brut de index.html (comme verifier-maj-atteignable-clavier
// et focus-interrupteur-segmente) : on veut geler le balisage source, pas un DOM
// reconstruit par jsdom.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";
import terminalOutils from "./terminal-outils.ts?raw";

/** Toutes les balises ouvrantes dont l'attribut class commence par `prefixe`. */
function balises(prefixe: string): string[] {
  const re = new RegExp(`<div\\b[^>]*\\bclass="${prefixe}[^"]*"[^>]*>`, "g");
  return indexHtml.match(re) ?? [];
}

describe("Les menus contextuels portent les rôles ARIA d'un menu", () => {
  it("les cinq conteneurs .ctx-menu portent role=\"menu\"", () => {
    const menus = balises("ctx-menu");
    // term, sftp, rdp, host, folder : cinq menus contextuels dans index.html.
    expect(menus.length).toBe(5);
    for (const m of menus) expect(m, m).toMatch(/\brole="menu"/);
  });

  it("chaque entrée .ctx-item porte role=\"menuitem\"", () => {
    const items = balises("ctx-item");
    // Le repère « Se connecter » lu comme simple texte venait de là.
    expect(items.length).toBeGreaterThan(0);
    for (const i of items) expect(i, i).toMatch(/\brole="menuitem"/);
  });

  it("chaque .ctx-sep porte role=\"separator\"", () => {
    const seps = balises("ctx-sep");
    expect(seps.length).toBeGreaterThan(0);
    for (const s of seps) expect(s, s).toMatch(/\brole="separator"/);
  });

  it("le grisage de « Copier » pose aria-disabled, pas seulement la classe", () => {
    // .ctx-item.disabled est invisible à un lecteur d'écran (opacity + classe) ;
    // sans aria-disabled synchronisé, l'entrée grisée était annoncée active.
    expect(terminalOutils).toMatch(/setAttribute\(\s*["']aria-disabled["']/);
  });
});
