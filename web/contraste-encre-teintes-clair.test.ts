// Trouvé par l'audit du 7 septembre 2026 : en thème clair, les jetons de teinte
// vive --accent-soft (#7c6bf5), --ok (#1a9d5f), --warn (#b0791f) et --err
// (#d6455a) servaient AUSSI d'encre de petits textes (10 à 12,5 px, jamais
// « large » au sens WCAG) et retombaient entre 2,6 et 3,8:1 sur les fonds clairs
// #eef0f5 (barre latérale, panneau SFTP) et #fff (cartes) : noms de dossiers
// SFTP (.sftp-entry.dir .nm), pastille de tag active (.tag-pill.on), onglet actif
// (.tabbar-btn.active), badge « rebond » (.host .jumptag), badge tunnel d'un hôte
// (.host .tun), statuts SFTP (.sftp-status.ok/.err), boutons de tunnel (.tbtn.go/
// .stop), erreur de tunnel (.tunnel-row .terr). L'audit axe ne voyait jamais ces
// états (il n'ouvrait que la vue principale sans panneau ni tag actif ni rebond),
// si bien qu'aucune régression n'était gardée. Trois pièges de couplage se
// cachaient derrière le simple assombrissement des jetons : --accent-soft sert
// aussi d'encre CLAIRE sur le voile sombre du glisser-déposer SFTP (.sftp-drop,
// fond toujours sombre) ; .host .jumptag avait opacity .85 qui rediluait le badge
// sous 4,5:1 ; .ctx-item:hover .ctx-key codait une encre sombre en dur, illisible
// sur l'accent en thème clair. Ces tests lisent le CSS de index.html et
// verrouillent l'AA (4,5:1) sur les fonds réellement rendus, dans les DEUX blocs
// clairs (@media prefers-color-scheme:light ET :root[data-theme="light"]).
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
 *  pour `pct %`. C'est ce que rend le navigateur pour une pastille teintée. */
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

describe("Contraste de l'encre des teintes vives en thème clair", () => {
  for (const [nom, sel] of blocsClairs) {
    it(`${nom} : encre de texte à l'AA (4,5:1) sur ses fonds clairs`, () => {
      const bloc = corpsRegle(source, sel);
      const bgSoft = hexVersRgb(jeton(bloc, "bg-soft")); // barre latérale, panneau SFTP
      const card = hexVersRgb(jeton(bloc, "card")); // cartes, boutons
      const accent = hexVersRgb(jeton(bloc, "accent")); // base de --accent-tint
      const soft = hexVersRgb(jeton(bloc, "accent-soft"));
      const ok = hexVersRgb(jeton(bloc, "ok"));
      const warn = hexVersRgb(jeton(bloc, "warn"));
      const err = hexVersRgb(jeton(bloc, "err"));

      // --accent-tint est rgb(accent .10) : les pastilles/onglets actifs le
      // posent sur la barre latérale. --accent-soft y est l'encre.
      const tintAccent = melangeSurFond(accent, 10, bgSoft);
      // .host .tun : encre --ok sur (--ok 14 %) posé sur la barre latérale.
      const tintTun = melangeSurFond(ok, 14, bgSoft);

      // .sftp-entry.dir .nm, .host .jumptag : --accent-soft sur #eef0f5.
      expect(contraste(soft, bgSoft)).toBeGreaterThanOrEqual(4.5);
      // .tag-pill.on, .tabbar-btn.active : --accent-soft sur la pastille teintée.
      expect(contraste(soft, tintAccent)).toBeGreaterThanOrEqual(4.5);
      // .host .tun : --ok sur son propre voile teinté (cas le plus serré).
      expect(contraste(ok, tintTun)).toBeGreaterThanOrEqual(4.5);
      // .sftp-status.ok : --ok directement sur #eef0f5.
      expect(contraste(ok, bgSoft)).toBeGreaterThanOrEqual(4.5);
      // .tbtn.stop : --warn sur une carte / la barre latérale.
      expect(contraste(warn, card)).toBeGreaterThanOrEqual(4.5);
      expect(contraste(warn, bgSoft)).toBeGreaterThanOrEqual(4.5);
      // .tunnel-row .terr, .sftp-transfert.erreur : --err sur une carte #fff.
      expect(contraste(err, card)).toBeGreaterThanOrEqual(4.5);
      // .sftp-status.err : --err sur #eef0f5.
      expect(contraste(err, bgSoft)).toBeGreaterThanOrEqual(4.5);
    });

    it(`${nom} : le raccourci du menu contextuel survolé passe l'AA`, () => {
      // .ctx-item:hover peint le fond en --accent et le libellé en --accent-ink ;
      // .ctx-key doit suivre la même encre pour rester lisible.
      const bloc = corpsRegle(source, sel);
      const accent = hexVersRgb(jeton(bloc, "accent"));
      const accentInk = hexVersRgb(jeton(bloc, "accent-ink"));
      expect(contraste(accentInk, accent)).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("les jetons ne portent plus les teintes vives d'origine en thème clair", () => {
    // Garde-fou direct : les valeurs fautives repérées par l'audit ne doivent
    // pas revenir (un futur ajustement de teinte les réintroduirait sans que les
    // ratios ci-dessus ne bougent s'il touchait un fond en même temps).
    for (const [, sel] of blocsClairs) {
      const bloc = corpsRegle(source, sel);
      expect(jeton(bloc, "accent-soft").toLowerCase()).not.toBe("#7c6bf5");
      expect(jeton(bloc, "ok").toLowerCase()).not.toBe("#1a9d5f");
      expect(jeton(bloc, "warn").toLowerCase()).not.toBe("#b0791f");
      expect(jeton(bloc, "err").toLowerCase()).not.toBe("#d6455a");
    }
  });

  it(".ctx-item:hover .ctx-key suit --accent-ink au lieu d'une encre sombre figée", () => {
    const corps = corpsRegle(source, "\\.ctx-item:hover \\.ctx-key");
    expect(corps).toMatch(/color:\s*var\(--accent-ink\)/);
    // L'ancienne valeur codée en dur (encre sombre, pensée pour le thème sombre)
    // tombait à 2,8:1 sur l'accent en thème clair.
    expect(corps).not.toMatch(/rgb\(\s*20\s*,\s*16\s*,\s*31/);
  });

  it(".host .jumptag n'atténue plus le badge (opacity supprimée)", () => {
    // opacity .85 rediluait --accent-soft assombri sous 4,5:1 sur la barre
    // latérale ; sans elle le badge tient l'AA.
    const corps = corpsRegle(source, "\\.host \\.jumptag");
    expect(corps).not.toMatch(/opacity\s*:/);
  });

  it(".sftp-drop garde une encre claire sur son voile sombre (découplée du thème)", () => {
    // Le voile du glisser-déposer est toujours sombre : son encre ne peut pas
    // suivre --accent-soft, assombri en thème clair, qui tomberait à ~3:1.
    const corps = corpsRegle(source, "\\.sftp-drop");
    expect(corps).not.toMatch(/color:\s*var\(--accent-soft\)/);
    const m = corps.match(/color:\s*(#[0-9a-fA-F]{3,6})/);
    if (!m) throw new Error(".sftp-drop : couleur d'encre littérale introuvable");
    const voile: Rgb = [11, 13, 20]; // background: rgb(11,13,20,.82)
    expect(contraste(hexVersRgb(m[1]), voile)).toBeGreaterThanOrEqual(4.5);
  });

  it("le jeu de sélecteurs reste bien indexé (garde-fou de parsing)", () => {
    expect(regles).toContain(".sftp-entry.dir .nm");
    expect(regles).toContain(".host .tun");
    expect(regles).toContain(".sftp-drop");
  });
});
