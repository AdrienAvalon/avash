// Audit d'accessibilité automatisé (axe-core), sur l'application RÉELLE.
//
// Les vérifications d'accessibilité écrites à la main (a11y.spec.js) couvrent
// ce à quoi on a pensé : rôles des modales, piège à focus, retour du focus.
// axe-core couvre ce à quoi on n'a pas pensé — contrastes insuffisants, noms
// accessibles manquants, régions sans repère, ordre de titres. C'est le même
// principe que le parc RDP : confronter à un juge extérieur plutôt qu'à sa
// propre compréhension.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const SOURCE_AXE = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");

// Règles retenues : celles qui portent sur des défauts réels et vérifiables
// dans une application de bureau. On écarte volontairement ce qui suppose un
// document web complet (langue de page, points de repère de navigation), sans
// rapport avec une fenêtre native.
const HORS_PERIMETRE = new Set([
  "html-has-lang",
  "landmark-one-main",
  "page-has-heading-one",
  "region",
]);

// Les couleurs relevées en thème clair tombaient exactement à mi-chemin entre
// les valeurs sombres et claires — #8d96ab → #737c91 → #5a6478. On mesurait
// PENDANT la transition CSS. Un audit de contraste doit lire un état stable,
// sinon il invente des violations qui n'existent dans aucune image affichée.
async function figerLesAnimations() {
  await browser.execute(() => {
    let s = document.getElementById("axe-fige");
    if (!s) {
      s = document.createElement("style");
      s.id = "axe-fige";
      s.textContent =
        "*,*::before,*::after{transition:none!important;animation:none!important}";
      document.head.appendChild(s);
    }
  });
}

async function auditer(contexte) {
  await figerLesAnimations();
  await browser.execute(SOURCE_AXE);
  const resultat = await browser.executeAsync((ctx, done) => {
    // eslint-disable-next-line no-undef
    window.axe
      .run(ctx || document, { resultTypes: ["violations"] })
      .then((r) =>
        done(
          r.violations.map((v) => ({
            id: v.id,
            impact: v.impact,
            aide: v.help,
            cibles: v.nodes.slice(0, 3).map((n) => n.target.join(" ")),
          })),
        ),
      )
      .catch((e) => done([{ id: "axe-erreur", impact: "critical", aide: String(e), cibles: [] }]));
  }, contexte);
  return resultat.filter((v) => !HORS_PERIMETRE.has(v.id));
}

function raconter(violations) {
  return violations
    .map((v) => `  [${v.impact}] ${v.id} — ${v.aide}\n      ${v.cibles.join("\n      ")}`)
    .join("\n");
}

describe("Audit d'accessibilité (axe-core)", () => {
  it("la vue principale ne présente aucune violation", async () => {
    await $("#host-list").waitForExist({ timeout: 15000 });
    const violations = await auditer(null);
    if (violations.length) console.log(`\n${raconter(violations)}`);
    expect(violations.map((v) => v.id)).toEqual([]);
  });

  // Le thème clair était PIRE que le sombre — 2,45:1 contre 3,15:1 — et aucun
  // test ne l'aurait montré : ils tournent tous en sombre. Un thème qu'on ne
  // regarde jamais est un thème où les défauts s'accumulent.
  it("le thème clair ne présente aucune violation non plus", async () => {
    await $("#host-list").waitForExist({ timeout: 15000 });
    // Passer par le bouton de thème, comme l'utilisateur. Poser `data-theme`
    // à la main ne tient pas : l'application repilote l'attribut depuis sa
    // préférence, et l'audit lisait alors une palette en train de changer.
    // Geler AVANT de basculer : geler après laisserait les couleurs figées au
    // milieu de la transition, ce qui fabrique des violations imaginaires.
    await figerLesAnimations();
    const bouton = await $("#theme-toggle");
    for (let i = 0; i < 4; i++) {
      const clair = await browser.execute(
        () => document.documentElement.getAttribute("data-theme") === "light");
      if (clair) break;
      await bouton.click();
    }
    expect(
      await browser.execute(() => document.documentElement.getAttribute("data-theme")),
    ).toBe("light");
    const violations = await auditer(null);
    for (let i = 0; i < 4; i++) {
      const sombre = await browser.execute(
        () => document.documentElement.getAttribute("data-theme") === "dark");
      if (sombre) break;
      await bouton.click();
    }
    if (violations.length) console.log(`\n${raconter(violations)}`);
    expect(violations.map((v) => v.id)).toEqual([]);
  });

  it("la boîte de connexion manuelle ne présente aucune violation", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    const violations = await auditer("#manual-modal");
    await browser.keys("Escape");
    if (violations.length) console.log(`\n${raconter(violations)}`);
    expect(violations.map((v) => v.id)).toEqual([]);
  });
});
