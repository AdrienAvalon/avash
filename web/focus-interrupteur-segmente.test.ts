// Trouvé par l'audit du 7 septembre 2026 : `.modal input:focus` posait
// `outline: none` sur TOUS les champs de la modale, radios cachées (0×0,
// opacity 0) et cases à cocher (15×15, bien visibles) comprises, écrasant
// l'anneau `:focus-visible` global (spécificité inférieure). Résultat : en
// tabulant sur les interrupteurs segmentés (SSH/RDP/VNC/Série, Mot de passe /
// Clé, Local/Distant/SOCKS) rien n'apparaissait tant qu'on n'appuyait pas sur
// une flèche, et les cases RDP « Mémoriser » / « Enregistrer » n'avaient plus
// aucun repère de focus (échec WCAG 2.4.7). Ces tests lisent le CSS de
// index.html et verrouillent que la règle exclut désormais radios et cases, et
// qu'une pastille de radio focalisée au clavier reçoit un anneau.
//
// On analyse le texte du bloc <style> plutôt que le CSSOM de jsdom : ce dernier
// (rrweb-cssom) abandonne silencieusement toute règle dont le sélecteur emploie
// `:not(a, b)` (liste d'arguments, niveau 4) — justement la forme de notre
// exclusion —, elle disparaîtrait de la feuille et le test passerait à tort. Le
// comportement réel (l'anneau qui s'affiche) est en plus vérifié par
// e2e/specs/a11y.spec.js sur l'application réelle.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";

/** Le texte de la feuille de style inline de index.html. */
function css(): string {
  const bloc = indexHtml.match(/<style>([\s\S]*?)<\/style>/);
  if (!bloc) throw new Error("aucun bloc <style> dans index.html");
  return bloc[1];
}

/** Les groupes de sélecteurs (le texte avant `{`) de chaque règle du CSS. */
function selecteurs(source: string): string[] {
  return [...source.matchAll(/([^{}]*)\{/g)]
    .map((m) => m[1].replace(/\/\*[\s\S]*?\*\//g, "").trim())
    .filter(Boolean);
}

describe("Focus clavier des interrupteurs segmentés et des cases de la modale", () => {
  const regles = selecteurs(css());

  it("la règle qui coupe l'anneau des champs de la modale exclut radios et cases", () => {
    // Sans cette exclusion, l'`outline: none` retirait aussi le focus-visible
    // des radios cachées et des cases à cocher visibles.
    const coupures = regles.filter((s) => /\.modal\s+input\b/.test(s) && /:focus\b/.test(s));
    expect(coupures.length).toBeGreaterThan(0);
    for (const s of coupures) {
      expect(s).toContain('[type="radio"]');
      expect(s).toContain('[type="checkbox"]');
    }
  });

  it("une pastille de l'interrupteur segmenté montre le focus clavier", () => {
    // C'est la règle qui rend visible la tabulation sur SSH/RDP/… : sans elle,
    // la radio étant 0×0 et opacity 0, aucun indicateur n'apparaissait.
    const anneau = regles.some((s) =>
      /\.auth-switch\s+\.radio:has\(input:focus-visible\)/.test(s),
    );
    expect(anneau).toBe(true);
  });
});
