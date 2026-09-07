// Contrôle reproductible : la relance du pilote WebDriver vit dans le LANCEUR.
//
// Trouvé par l'audit du 7 septembre 2026 : `relancerPiloteSiMort`
// (e2e/wdio.conf.js) était appelé depuis `beforeSession`, hook qui tourne dans
// le PROCESSUS DE TRAVAIL (@wdio/runner), alors que la poignée `tauriDriver` et
// le compteur `relances` sont posés par onPrepare dans le LANCEUR (@wdio/cli).
// Dans le travailleur, `tauriDriver` valait undefined (le kill ne tuait rien),
// `relances` repartait de zéro à chaque fichier, et le tauri-driver relancé
// appartenait au travailleur : onComplete (lanceur) ne le tuait jamais et un
// pilote orphelin gardait le port 4444, empoisonnant le run suivant.
//
// Le correctif déplace la relance dans le hook de lanceur `onWorkerStart` et la
// retire de `beforeSession`. Ce test le vérifie sans dépendre d'un vrai
// tauri-driver : (1) `onWorkerStart` doit exister (il n'existait pas avant) ;
// (2) hors chemin embarqué, `beforeSession` ne doit plus toucher au pilote — il
// se contente de resemer le bac à sable et rend la main aussitôt, là où
// l'ancien appelait la relance (qui, pilote absent, bloquait vingt secondes
// puis levait). Contre l'ancien harnais, l'assertion (1) échoue tout court.
import test from "node:test";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mkdtempSync } from "node:fs";

// Bac à sable réel désigné : l'import ne doit pas en créer un autre, et
// `beforeSession` (via seedSandbox) écrit dedans.
const sandbox = mkdtempSync(join(tmpdir(), "avash-e2e-relance-"));
process.env.AVASH_E2E_SANDBOX = sandbox;
// On force le chemin NON embarqué (celui du pilote tauri-driver) : c'est lui que
// le défaut concernait ; le chemin embarqué lance l'application autrement.
delete process.env.E2E_EMBARQUE;

const { config } = await import("../../e2e/wdio.conf.js");

test("la relance du pilote est un hook de lanceur (onWorkerStart), pas de travailleur", () => {
  // onWorkerStart tourne dans @wdio/cli avant chaque travailleur, là où vivent
  // la poignée tauriDriver et relances. L'ancien harnais n'avait pas ce hook :
  // la relance était coincée dans beforeSession (travailleur), impuissante.
  assert.equal(typeof config.onWorkerStart, "function",
    "onWorkerStart (hook de lanceur) doit porter la vérification/relance du pilote");
});

test("beforeSession ne relance plus le pilote depuis le travailleur", async (t) => {
  // Sous Windows ou en embarqué, beforeSession lance l'application lui-même :
  // ce test ne vise que le chemin tauri-driver.
  if (process.platform === "win32" || config.port === 4445) {
    t.skip("chemin embarqué : pas de pilote tauri-driver à relancer ici");
    return;
  }
  // Dans l'ancien harnais, beforeSession appelait relancerPiloteSiMort ; sans
  // pilote sur 4444, celui-ci bouclait ~20 s avant de lever. Dans le harnais
  // corrigé, beforeSession ne fait que resemer le bac à sable et rend la main
  // en quelques millisecondes, sans jamais spawner de tauri-driver.
  const debut = Date.now();
  await config.beforeSession();
  const ecoule = Date.now() - debut;
  assert.ok(ecoule < 5000,
    `beforeSession ne doit plus piloter la relance (rendu en ${ecoule} ms ; l'ancien bloquait ~20 s pilote absent)`);
});
