// Trouvé par l'audit du 7 septembre 2026 : renderPalette (main.ts) posait
// role="option" et aria-selected sur chaque résultat, et aria-activedescendant
// sur l'input, mais #palette-input n'était pas un role="combobox" et
// #palette-results n'était pas un role="listbox". ARIA exige un parent listbox
// pour des options : axe rapportait aria-required-parent (impact serious) sur la
// palette ouverte, et un lecteur d'écran (NVDA/Orca) lisait le champ comme une
// simple zone de texte, sans liste associée ni annonce de la sélection courante.
// De plus, la branche « aucun hôte » laissait aria-activedescendant pointer vers
// une option retirée du DOM, et rien ne pilotait aria-expanded.
//
// On analyse le texte brut de index.html et de main.ts (comme
// menus-contextuels-roles-aria et verifier-maj-atteignable-clavier) : on veut
// geler le balisage source et le câblage ARIA, pas un DOM reconstruit par jsdom.
// L'audit axe réel de la palette ouverte est ajouté en bout à bout
// (e2e/specs/axe.spec.js) ; ici on verrouille la source.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";
import mainTs from "./main.ts?raw";

/** La balise ouvrante de l'élément d'id `id` dans index.html. */
function baliseParId(id: string): string {
  const m = indexHtml.match(new RegExp(`<[a-z]+\\b[^>]*\\bid="${id}"[^>]*>`));
  expect(m, `balise #${id} introuvable`).not.toBeNull();
  return m![0];
}

describe("La palette forme un combobox/listbox valide pour les lecteurs d'écran", () => {
  it("#palette-input est un combobox qui pilote #palette-results", () => {
    const input = baliseParId("palette-input");
    // Sans role="combobox" + aria-controls, les role="option" enfants sont
    // orphelins : axe rapporte aria-required-parent.
    expect(input, input).toMatch(/\brole="combobox"/);
    expect(input, input).toMatch(/\baria-autocomplete="list"/);
    expect(input, input).toMatch(/\baria-controls="palette-results"/);
    expect(input, input).toMatch(/\baria-expanded=/);
  });

  it("#palette-results est un listbox nommé", () => {
    const results = baliseParId("palette-results");
    expect(results, results).toMatch(/\brole="listbox"/);
    // Un nom accessible (via data-i18n-aria) pour que la liste soit annoncée.
    expect(results, results).toMatch(/\bdata-i18n-aria=/);
  });

  it("aria-expanded suit l'ouverture et la fermeture de la palette", () => {
    // paletteOpen doit annoncer « développé », paletteClose « replié ».
    expect(mainTs).toMatch(/setAttribute\(\s*["']aria-expanded["']\s*,\s*["']true["']/);
    expect(mainTs).toMatch(/setAttribute\(\s*["']aria-expanded["']\s*,\s*["']false["']/);
  });

  it("aria-activedescendant est retiré quand la liste est vide ou fermée", () => {
    // Deux retraits attendus : branche « aucun hôte » de renderPalette et
    // paletteClose, sinon l'input désigne une option absente du DOM.
    const retraits = mainTs.match(/removeAttribute\(\s*["']aria-activedescendant["']\s*\)/g) ?? [];
    expect(retraits.length, `retraits trouvés : ${retraits.length}`).toBeGreaterThanOrEqual(2);
  });
});
