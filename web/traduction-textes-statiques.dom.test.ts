// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : remplacerTexte (i18n.ts) ne remplace
// que le PREMIER nœud texte porteur de lettres d'un élément data-i18n. La fin de
// plusieurs modal-hint (le texte APRÈS un <code>), le « ou » entre les deux
// <code> du rebond ProxyJump et l'astuce de la commande snippet restaient donc
// en français en interface anglaise ; par ailleurs les cinq légendes de
// raccourcis de l'écran d'accueil, la zone de dépôt SFTP et le mot « bureau » de
// la connexion directe n'avaient aucune clé. Ce test bascule la page en anglais
// et exige qu'aucun texte français ne subsiste hors <code>/<kbd>.
import { describe, it, expect, beforeAll } from "vitest";
import indexHtml from "./index.html?raw";

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let setLangue: (l: "fr" | "en") => void;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  const i18n = await import("./i18n");
  setLangue = i18n.setLangue;
  setLangue("en"); // setLangue appelle appliquerLangue() sur toute la page
});

// Un accent français, ou un mot français dont aucun n'est aussi un mot anglais :
// le signe qu'un fragment n'a pas traversé la traduction. « panneau » est ajouté
// car « panneau SFTP » ne porte ni accent ni mot de la première liste (le mot
// « bureau » reste, lui, dans un <code> exclu du balayage : vérifié à part).
const MOTIF_FR = /[àâäéèêëîïôöûüçœ]|\b(ou|pour|les|dans|fermer|onglet|panneau|Déposer)\b/i;

describe("bascule de langue : tous les textes statiques de la page traversent la traduction", () => {
  it("aucun texte français ne subsiste hors <code>/<kbd> après « Switch to English »", () => {
    const fautes: string[] = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const parent = (n as Text).parentElement;
      // Le CSS, le code et les identifiants techniques ne se traduisent pas.
      if (!parent || parent.closest("code, kbd, style, script")) continue;
      const v = n.nodeValue ?? "";
      if (MOTIF_FR.test(v)) fautes.push(v.trim());
    }
    // Aurait listé les fins de modal-hint (import, clés SSH, snippets), le « ou »
    // du rebond, l'astuce de la commande snippet, les cinq légendes de raccourcis
    // et la zone de dépôt SFTP.
    expect(fautes).toEqual([]);
  });

  it("le mot « bureau » (dans un <code>) et le raccourci « palette » sont traduits", () => {
    // Deux cas que le balayage des nœuds texte ne voit pas : « bureau » vit dans
    // un <code> (exclu du balayage), et « palette »/« command palette » ne porte
    // aucun marqueur français.
    expect(document.querySelector('[data-i18n="bureau-rdp"]')?.textContent).toBe("desktop");
    expect(document.querySelector(".shortcuts span")?.textContent).toContain("command palette");
  });
});
