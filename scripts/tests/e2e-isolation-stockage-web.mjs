// Contrôle reproductible de l'isolation du STOCKAGE WEB dans la suite E2E.
//
// Trouvé par l'audit du 7 septembre 2026 : `seedSandbox` (e2e/wdio.conf.js) ne
// remettait à zéro que `.config/avash` et la config `.ssh`. Or la webview
// (WebKitGTK/wry, dont Tauri résout le répertoire par BaseDirectory::LocalData)
// range langue, partage de presse-papiers, santé, thème, largeurs, dossiers
// repliés et cache des logos dans son stockage web, sous les répertoires XDG de
// données/état/cache — jamais sous `.config`. Deux conséquences : le stockage
// web n'était pas remis à zéro entre fichiers (avash.sante fuyait, un échec de
// langue.spec après « Switch to English » démarrait les suivants en anglais car
// avash.langue prime sur AVASH_LANGUE dans lireLangue), et sur un poste où
// XDG_DATA_HOME est exporté la suite écrivait dans les données RÉELLES de
// l'utilisateur (même identifiant dev.avash.app).
//
// Ce test rejoue le vrai harnais : il vérifie que ENV_APP redirige les trois
// variables XDG dans le bac à sable, puis que seedSandbox efface bien un fichier
// semé sous ces répertoires. Contre l'ancien harnais (pas de XDG dans ENV_APP,
// pas de rmSync des répertoires XDG dans seedSandbox), il échoue.
import test from "node:test";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mkdtempSync, mkdirSync, writeFileSync, existsSync } from "node:fs";

// On DÉSIGNE le bac à sable pour que l'import du harnais n'en crée pas un autre :
// les processus de travail retrouvent ainsi le même chemin, et notre test opère
// sur celui-là.
const sandbox = mkdtempSync(join(tmpdir(), "avash-e2e-stockage-"));
process.env.AVASH_E2E_SANDBOX = sandbox;

const { ENV_APP, seedSandbox } = await import("../../e2e/wdio.conf.js");

test("ENV_APP redirige le stockage web (XDG data/cache/state) dans le bac à sable", () => {
  // Sans ces redirections, un poste où XDG_DATA_HOME est exporté ferait écrire
  // la webview dans les données réelles de l'utilisateur (même dev.avash.app).
  assert.equal(ENV_APP.XDG_DATA_HOME, join(sandbox, ".local", "share"),
    "XDG_DATA_HOME doit pointer dans le bac à sable, pas dans le répertoire de données réel");
  assert.equal(ENV_APP.XDG_CACHE_HOME, join(sandbox, ".cache"),
    "XDG_CACHE_HOME doit pointer dans le bac à sable");
  assert.equal(ENV_APP.XDG_STATE_HOME, join(sandbox, ".local", "state"),
    "XDG_STATE_HOME doit pointer dans le bac à sable");
});

test("seedSandbox efface le stockage web laissé par un fichier précédent", () => {
  // On simule les données que la webview écrit sous XDG_DATA_HOME (avash.langue,
  // avash.sante…) : un fichier quelconque sous le répertoire de l'application.
  const dossierWeb = join(sandbox, ".local", "share", "dev.avash.app");
  mkdirSync(dossierWeb, { recursive: true });
  const temoin = join(dossierWeb, "localstorage.sqlite");
  writeFileSync(temoin, "avash.langue=en\navash.sante=...");
  assert.ok(existsSync(temoin), "le témoin doit exister avant le semage");

  seedSandbox();

  assert.ok(!existsSync(temoin),
    "seedSandbox doit effacer le stockage web (XDG data) pour que chaque fichier reparte de l'état semé");
});
