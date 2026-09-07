// Trouvé par l'audit du 7 septembre 2026 : les couleurs de texte des messages
// d'erreur et de succès des modales étaient codées en dur en teintes pâles
// (`.modal-error { color: #ff9a9a }`, `.modal-ok { color: #8ee0ac }`, et
// `.key-row .kmode.warn { color: #ff9a9a }`), et les deux blocs « thème clair »
// ne les redéfinissaient pas. Posées sur le fond clair de la modale (--panel
// #fff, teinté à 12 % de --err/--ok), elles retombaient à ~1,7 / ~1,4 / ~2,0:1,
// quasi illisibles — or ce sont les textes critiques : refus de connexion,
// alias déjà pris, clé créée, « OpenSSH exige 600 sur une clé privée ». En
// sombre elles restent correctes (~7,5 / ~9:1). Ces tests lisent le CSS de
// index.html et verrouillent que les trois règles passent par les jetons
// --err-text / --ok-text et que ces jetons, dans les DEUX blocs clairs
// (@media prefers-color-scheme:light ET :root[data-theme="light"]), atteignent
// le contraste AA (4,5:1) sur les fonds effectivement rendus.
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

/** Lit la valeur d'une variable CSS (`--nom: valeur;`) dans un corps de règle. */
function jeton(corps: string, nom: string): string {
  const m = corps.match(new RegExp("--" + nom + "\\s*:\\s*([^;]+);"));
  if (!m) throw new Error(`jeton --${nom} absent`);
  return m[1].trim();
}

type Rgb = [number, number, number];

function hexVersRgb(hex: string): Rgb {
  const h = hex.replace("#", "").trim();
  const n =
    h.length === 3
      ? h
          .split("")
          .map((c) => c + c)
          .join("")
      : h;
  return [
    parseInt(n.slice(0, 2), 16),
    parseInt(n.slice(2, 4), 16),
    parseInt(n.slice(4, 6), 16),
  ];
}

/** Reproduit `color-mix(in srgb, teinte pct%, transparent)` posé sur un fond
 *  opaque : le transparent laisse voir le fond, la teinte n'y contribue que
 *  pour `pct %`. C'est exactement ce que rend le navigateur pour la boîte. */
function melangeSurFond(teinte: Rgb, pct: number, fond: Rgb): Rgb {
  const a = pct / 100;
  return [0, 1, 2].map((i) =>
    Math.round(teinte[i] * a + fond[i] * (1 - a)),
  ) as Rgb;
}

function luminance([r, g, b]: Rgb): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function contraste(a: Rgb, b: Rgb): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

const source = css();
const regles = [...source.matchAll(/([^{}]*)\{/g)]
  .map((m) => m[1].replace(/\/\*[\s\S]*?\*\//g, "").trim())
  .filter(Boolean);

// Les deux blocs qui décrivent le thème clair. Il faut corriger les DEUX,
// sinon le suivi système (media) ou le forçage manuel (data-theme) reste fautif.
const blocsClairs: Array<[string, string]> = [
  ["@media prefers-color-scheme:light", ':root:not\\(\\[data-theme="dark"\\]\\)'],
  [":root[data-theme=\"light\"]", ':root\\[data-theme="light"\\]'],
];

describe("Contraste des messages d'erreur et de succès des modales", () => {
  it("les trois règles colorent leur texte par un jeton, pas par un hex figé", () => {
    // Avant le correctif, `color: #ff9a9a` / `#8ee0ac` étaient écrits en dur et
    // survivaient donc au thème clair. On exige le passage par --err-text /
    // --ok-text pour que les blocs clairs puissent les assombrir.
    const modalError = corpsRegle(source, "\\.modal-error");
    const modalOk = corpsRegle(source, "\\.modal-ok");
    const kmodeWarn = corpsRegle(source, "\\.key-row \\.kmode\\.warn");
    expect(modalError).toMatch(/color:\s*var\(--err-text\)/);
    expect(modalOk).toMatch(/color:\s*var\(--ok-text\)/);
    expect(kmodeWarn).toMatch(/color:\s*var\(--err-text\)/);
    for (const corps of [modalError, modalOk, kmodeWarn]) {
      expect(corps).not.toMatch(/color:\s*#[0-9a-fA-F]{3,6}/);
    }
  });

  for (const [nom, sel] of blocsClairs) {
    it(`${nom} : les messages passent l'AA (4,5:1) sur leur fond clair`, () => {
      const bloc = corpsRegle(source, sel);
      const panel = hexVersRgb(jeton(bloc, "panel"));
      const card = hexVersRgb(jeton(bloc, "card"));
      const err = hexVersRgb(jeton(bloc, "err"));
      const ok = hexVersRgb(jeton(bloc, "ok"));
      const errText = hexVersRgb(jeton(bloc, "err-text"));
      const okText = hexVersRgb(jeton(bloc, "ok-text"));

      // .modal-error : texte sur (err 12 %) posé sur --panel (fond de .modal).
      const fondErr = melangeSurFond(err, 12, panel);
      // .modal-ok : texte sur (ok 12 %) posé sur --panel.
      const fondOk = melangeSurFond(ok, 12, panel);
      // .key-row .kmode.warn : texte directement sur --card (fond de .key-row).
      expect(contraste(errText, fondErr)).toBeGreaterThanOrEqual(4.5);
      expect(contraste(okText, fondOk)).toBeGreaterThanOrEqual(4.5);
      expect(contraste(errText, card)).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("le jeu de sélecteurs reste bien indexé (garde-fou de parsing)", () => {
    expect(regles).toContain(".modal-error");
    expect(regles).toContain(".key-row .kmode.warn");
  });
});
