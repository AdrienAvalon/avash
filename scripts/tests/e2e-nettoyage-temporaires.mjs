// Contrôle reproductible : le harnais et les specs suppriment leurs fichiers
// temporaires en fin de course.
//
// Trouvé par l'audit du 7 septembre 2026 : onComplete (e2e/wdio.conf.js) tuait
// tauri-driver et le sshd mais ne supprimait jamais le bac à sable créé par
// onPrepare (clé privée cliente, clé d'hôte, sshd.log, sshd_config). Et
// plusieurs specs créaient leur propre mkdtempSync sans `after` de nettoyage :
// sftp.spec.js (arbre SFTP), rdp-fichiers.spec.js (2,5 Mo + 0,3 Mo offerts, plus
// les copies reçues des deux côtés : ~5,6 Mo par run), vnc-tls.spec.js (le
// dossier du certificat rejoué) et serie.spec.js — seul rdp-lecteur.spec.js
// nettoyait. Sur un poste où /tmp est un tmpfs, chaque run consommait de la
// mémoire vive jusqu'au redémarrage (8 répertoires avash-*, 12 Mo au moment de
// l'audit). La garde exige que onComplete supprime le bac à sable (sauf
// E2E_GARDER_SANDBOX pour le débogage) et que chaque spec concernée efface sa
// racine temporaire dans un `after`, sur le modèle de rdp-lecteur.spec.js.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const ici = dirname(fileURLToPath(import.meta.url));
const racine = resolve(ici, "../..");
const lire = (rel) => readFileSync(resolve(racine, rel), "utf8");

const conf = lire("e2e/wdio.conf.js");

test("onComplete supprime le bac à sable, sauf E2E_GARDER_SANDBOX", () => {
  // Le corps de onComplete : de sa clé jusqu'à la fermeture de l'objet config.
  const debut = conf.indexOf("onComplete:");
  assert.ok(debut !== -1, "onComplete doit exister dans wdio.conf.js");
  const corps = conf.slice(debut);
  assert.match(corps, /rmSync\(sandbox,\s*\{\s*recursive:\s*true,\s*force:\s*true\s*\}\)/,
    "onComplete doit supprimer le bac à sable (rmSync(sandbox, { recursive, force }))");
  assert.match(corps, /E2E_GARDER_SANDBOX/,
    "onComplete doit conserver le bac à sable si E2E_GARDER_SANDBOX est posé (débogage)");
});

// Chaque spec : la variable racine de sa mkdtempSync doit être effacée dans un
// `after`. On vérifie la présence d'un `after(` et d'un rmSync récursif+forcé sur
// cette variable, sur le modèle de rdp-lecteur.spec.js:after.
for (const { fichier, variable } of [
  { fichier: "e2e/specs/sftp.spec.js", variable: "racine" },
  { fichier: "e2e/specs/rdp-fichiers.spec.js", variable: "racine" },
  { fichier: "e2e/specs/vnc-tls.spec.js", variable: "dossierCert" },
  { fichier: "e2e/specs/serie.spec.js", variable: "dossier" },
]) {
  test(`${fichier} : sa racine temporaire est nettoyée dans un after`, () => {
    const src = lire(fichier);
    assert.match(src, /after\(/,
      `${fichier} doit avoir un hook after pour nettoyer ses temporaires`);
    const motif = new RegExp(`rmSync\\(${variable},\\s*\\{\\s*recursive:\\s*true,\\s*force:\\s*true\\s*\\}\\)`);
    assert.match(src, motif,
      `${fichier} doit supprimer ${variable} (rmSync récursif et forcé) en fin de suite`);
  });
}
