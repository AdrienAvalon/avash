// Contrôle reproductible : en CI, une référence visuelle absente doit faire
// ROUGIR l'étape, pas être auto-sauvée en silence.
//
// Trouvé par l'audit du 7 septembre 2026 : le service visuel de wdio.conf.js
// portait `autoSaveBaseline: true` en dur, y compris quand process.env.CI est
// posé. Quand une capture n'a pas de référence dans e2e/visuel/reference,
// @wdio/image-comparison-core l'enregistre comme référence et checkScreen rend
// 0 : le test passe. Un tag ajouté ou renommé dans visuel.spec.js sans commiter
// son PNG était donc vert à chaque run (l'exécuteur est neuf, la référence
// refabriquée) et ne comparait jamais rien. Seuls les écarts sur les quatre
// références déjà commises étaient encore détectés.
//
// Le correctif : `autoSaveBaseline: !process.env.CI || !!process.env.VISUEL_INIT`
// (en CI une référence absente lève « Baseline image not found » et l'étape
// rougit ; VISUEL_INIT=1 amorce volontairement, en local ou en workflow_dispatch)
// et `alwaysSaveActualImage: true` (sans quoi, autoSaveBaseline off et référence
// absente, l'image réelle n'est pas écrite sur disque et l'artefact
// e2e/.tmp/visuel ne fournit pas le PNG à commiter au contributeur).
//
// Ce test recharge la vraie config sous plusieurs environnements et lit les
// options passées au service « visual ». Contre l'ancien harnais, le cas CI
// (autoSaveBaseline attendu à false) et l'exigence alwaysSaveActualImage
// échouent tous les deux.
import test from "node:test";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mkdtempSync } from "node:fs";

// Bac à sable réel désigné : chaque rechargement de la config n'en crée pas un
// nouveau (le module fait mkdtempSync au chargement si la variable manque).
process.env.AVASH_E2E_SANDBOX = mkdtempSync(join(tmpdir(), "avash-e2e-visuel-"));
// VISUEL=1 : sans quoi `services` est vide et il n'y a rien à inspecter.
process.env.VISUEL = "1";
// On ne pilote pas le chemin embarqué : la config du service visuel en est
// indépendante, mais on reste déterministe.
delete process.env.E2E_EMBARQUE;

// Recharge la config avec l'environnement courant (import ESM mis en cache par
// spécificateur : la chaîne de requête force un module neuf à chaque appel) et
// renvoie les options du service « visual ».
async function optionsVisuel(marqueur) {
  const { config } = await import(`../../e2e/wdio.conf.js?v=${marqueur}`);
  const service = config.services.find((s) => Array.isArray(s) && s[0] === "visual");
  assert.ok(service, "le service « visual » doit être chargé sous VISUEL=1");
  return service[1];
}

test("en CI, une référence absente n'est PAS auto-sauvée (l'étape peut rougir)", async () => {
  process.env.CI = "true";
  delete process.env.VISUEL_INIT;
  const opts = await optionsVisuel("ci-sans-init");
  assert.equal(opts.autoSaveBaseline, false,
    "autoSaveBaseline doit être faux en CI : une référence manquante doit lever « Baseline image not found », pas se refabriquer");
});

test("VISUEL_INIT=1 amorce volontairement les références même en CI", async () => {
  process.env.CI = "true";
  process.env.VISUEL_INIT = "1";
  const opts = await optionsVisuel("ci-avec-init");
  assert.equal(opts.autoSaveBaseline, true,
    "VISUEL_INIT=1 doit réactiver l'auto-sauvegarde pour amorcer volontairement (workflow_dispatch, ou local)");
});

test("hors CI, l'auto-sauvegarde reste active (références locales dans .tmp)", async () => {
  delete process.env.CI;
  delete process.env.VISUEL_INIT;
  const opts = await optionsVisuel("hors-ci");
  assert.equal(opts.autoSaveBaseline, true,
    "en local, autoSaveBaseline reste vrai : les références de la machine vont dans le dossier ignoré .tmp/visuel-local");
});

test("l'image réelle est toujours écrite sur disque (artefact exploitable)", async () => {
  process.env.CI = "true";
  delete process.env.VISUEL_INIT;
  const opts = await optionsVisuel("actual-sur-disque");
  assert.equal(opts.alwaysSaveActualImage, true,
    "alwaysSaveActualImage doit être vrai : sinon, référence absente et autoSaveBaseline off, l'artefact e2e/.tmp/visuel ne contient pas le PNG à commiter");
});
