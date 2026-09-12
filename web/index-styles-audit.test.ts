// Règles de style de index.html relevées par l'audit du 12 septembre 2026.
//
// On lit le texte brut de la page, comme contraste-encre-teintes-clair.test.ts :
// ce qui est verrouillé ici, ce sont des décisions de style, pas un rendu.
//  - C-front-10 : les ascenseurs étaient codés en dur (#262c3d, sombres sur un
//    panneau blanc en thème clair) ; le HUD du bureau distant portait un
//    `backdrop-filter` recalculé à chaque trame RDP, en logiciel sous WebKitGTK
//    sans compositing.
//  - C-front-11 : la croix d'onglet (18 px, invisible hors survol) et les
//    pastilles de tag (~20 px) étaient sous les 24 px de WCAG 2.5.8.
//  - C-front-8 : la police régulière n'était pas préchargée, et `font-display:
//    block` masquait jusqu'à 3 s tout texte en --mono si elle tardait.
//  - C-SIL-3 : un voyant de sonde périmé doit se voir comme tel.
//  - C-front-15 : l'incrustation « connexion en cours » d'un bureau a son style.
import { describe, it, expect } from "vitest";
import indexHtml from "./index.html?raw";

const css = indexHtml.slice(indexHtml.indexOf("<style>"), indexHtml.indexOf("</style>"));

/** Corps de la première règle dont le sélecteur est exactement `sel`. */
function regle(sel: string): string {
  const echappe = sel.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const m = css.match(new RegExp(`(?:^|[}\\s])${echappe}\\s*\\{([^}]*)\\}`));
  expect(m, `règle « ${sel} » introuvable`).not.toBeNull();
  return m![1];
}
const px = (corps: string, prop: string): number => Number(new RegExp(`(?:^|[;\\s])${prop}\\s*:\\s*(\\d+(?:\\.\\d+)?)px`).exec(corps)?.[1] ?? NaN);

describe("thème et mouvement", () => {
  it("les_ascenseurs_suivent_le_theme", () => {
    const regles = [...css.matchAll(/([^{}]*::-webkit-scrollbar[^{}]*)\{([^}]*)\}/g)];
    expect(regles.length).toBeGreaterThan(0);
    for (const [, sel, corps] of regles) {
      expect(corps, `couleur codée en dur dans « ${sel.trim()} »`).not.toMatch(/background\s*:\s*#/);
    }
    // Le jeton existe en sombre et il est redéfini dans les deux blocs clairs.
    const definitions = css.match(/--scroll-thumb\s*:/g) ?? [];
    expect(definitions.length).toBeGreaterThanOrEqual(3);
  });

  it("le_hud_du_bureau_ne_floute_pas_a_chaque_trame", () => {
    expect(regle(".rdp-hud")).not.toMatch(/backdrop-filter/);
  });
});

describe("cibles de pointage (WCAG 2.5.8)", () => {
  it("la_croix_d_onglet_fait_au_moins_24_px_et_se_voit_au_repos", () => {
    const croix = regle(".tab .close");
    expect(px(croix, "width")).toBeGreaterThanOrEqual(24);
    expect(px(croix, "height")).toBeGreaterThanOrEqual(24);
    const opacite = Number(/opacity\s*:\s*([\d.]+)/.exec(croix)?.[1] ?? "0");
    expect(opacite).toBeGreaterThan(0);
  });

  it("les_pastilles_de_tag_font_au_moins_24_px_de_haut", () => {
    expect(px(regle(".tag-pill"), "min-height")).toBeGreaterThanOrEqual(24);
  });
});

describe("polices", () => {
  it("la_police_reguliere_est_prechargee", () => {
    const tete = indexHtml.slice(0, indexHtml.indexOf("<style>"));
    expect(tete).toMatch(/<link rel="preload" href="\/fonts\/avash-mono-regular\.woff2" as="font" type="font\/woff2" crossorigin>/);
  });

  it("aucune_face_ne_masque_le_texte_en_attendant_la_police", () => {
    const faces = [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((m) => m[1]);
    expect(faces.length).toBe(2);
    for (const f of faces) expect(f).toMatch(/font-display:\s*swap/);
  });
});

describe("états visibles", () => {
  it("un_voyant_de_sonde_perime_a_son_style", () => {
    expect(css).toMatch(/\.dot\.stale\s*[,{]/);
  });

  it("l_incrustation_de_connexion_d_un_bureau_a_son_style", () => {
    expect(css).toMatch(/\.rdp-connexion\s*[,{]/);
  });
});
