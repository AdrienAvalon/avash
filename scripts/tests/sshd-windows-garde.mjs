// Contrôle reproductible de la garde du sshd SYSTÈME sous Windows.
//
// Trouvé par l'audit du 7 septembre 2026 : sous Windows, `preparerSshdWindows`
// (e2e/wdio.conf.js) démarre le service sshd du SYSTÈME, remplace le contenu de
// C:\ProgramData\ssh\administrators_authorized_keys par la clé du bac à sable,
// réécrit ses ACL et redémarre le service. Rien n'exigeait la CI : le seul
// `WINDOWS && LOCAL_SERVERS` suffisait, c'est-à-dire un `npm test` dans un
// terminal élevé. Un contributeur Windows perdait alors ses clés d'admin et
// laissait la clé de test autorisée sur le port 22 réel de sa machine.
//
// La garde doit refuser ce chemin hors CI (ou E2E_SSHD_SYSTEME posé en
// connaissance de cause), AVANT tout démarrage de service : on le vérifie ici
// en constatant que l'erreur levée nomme bien E2E_SSHD_SYSTEME, et non l'échec
// tardif d'un `Start-Service` (powershell) qui prouverait que le chemin système
// a déjà été emprunté.
import test from "node:test";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Éviter que l'import du harnais ne crée un vrai bac à sable temporaire : on lui
// en désigne un (jamais créé sur le disque, la garde tranche avant tout accès).
process.env.AVASH_E2E_SANDBOX ??= join(tmpdir(), "avash-e2e-garde-test");

test("preparerSshdWindows refuse de modifier le sshd système hors CI", async () => {
  const { CI, E2E_SSHD_SYSTEME } = process.env;
  delete process.env.CI;
  delete process.env.E2E_SSHD_SYSTEME;
  try {
    const { preparerSshdWindows } = await import("../../e2e/wdio.conf.js");
    assert.equal(
      typeof preparerSshdWindows,
      "function",
      "preparerSshdWindows doit être exporté pour être vérifiable",
    );
    assert.throws(
      () => preparerSshdWindows("clef.pub"),
      /E2E_SSHD_SYSTEME/,
      "sans garde, le chemin Windows écrase administrators_authorized_keys du système au lieu de refuser",
    );
  } finally {
    if (CI !== undefined) process.env.CI = CI;
    if (E2E_SSHD_SYSTEME !== undefined) process.env.E2E_SSHD_SYSTEME = E2E_SSHD_SYSTEME;
  }
});
