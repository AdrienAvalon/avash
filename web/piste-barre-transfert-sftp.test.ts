// Trouvé par l'audit du 7 septembre 2026 : `.sftp-transfert .barre` peignait sa
// piste avec `var(--card-2, rgb(255 255 255 / 8%))`. Le jeton `--card-2`
// n'existe dans aucun bloc de thème (grep négatif hors cette ligne), donc le
// repli s'appliquait toujours : un blanc à 8 % posé sur le panneau SFTP clair
// (--bg-soft #eef0f5 en thème clair) est invisible. En thème clair, un envoi de
// plusieurs Mo ne montrait qu'un fragment violet grandissant sur rien, et un
// transfert à 0 % ou en attente n'avait plus aucune barre. Le correctif remplace
// le repli par une teinte qui suit le thème (color-mix sur --text, défini dans
// les deux thèmes) et supprime la référence morte à --card-2. Ce test lit le CSS
// de index.html et verrouille la règle.
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

describe("piste de la barre de progression SFTP", () => {
  it("n'utilise pas le jeton --card-2, jamais défini dans le dépôt", () => {
    // `--card-2` n'apparaît nulle part ailleurs : la référence était morte et le
    // repli blanc à 8 % gagnait toujours.
    expect(source).not.toMatch(/--card-2/);
  });

  it("peint sa piste avec une teinte qui suit le thème", () => {
    // color-mix sur --text (#171b26 en clair, clair en sombre) donne un contraste
    // visible sur le panneau clair comme sur #191e2c, y compris à 0 % ou en attente.
    const bloc = corpsRegle(source, "\\.sftp-transfert \\.barre");
    expect(bloc).toMatch(/color-mix\(\s*in srgb\s*,\s*var\(--text\)/);
  });
});
