// Contrôle reproductible : le sshd du harnais est vérifié à son démarrage.
//
// Trouvé par l'audit du 8 septembre 2026 : startSshd (e2e/wdio.conf.js) spawnait
// `sshd -D` avec stdio ignoré et sans écouteur `exit`, et onPrepare n'attendait
// ni le port ni le fichier PID. Si le port 2223 était déjà tenu par un sshd
// orphelin d'un run précédent (lanceur tué sans onComplete — SIGKILL, timeout CI
// local), NOTRE sshd neuf mourait en silence sur le bind et l'orphelin —
// configuré avec l'authorized_keys d'un AUTRE bac à sable — répondait à sa
// place : ssh, sftp, enregistrement, restauration, vue-partagee, tunnels, sante
// et enregistrer-et-connecter échouaient tous « jamais live » / « Permission
// denied » sans qu'aucun message ne nomme le port occupé.
//
// Deux temps : (1) démontrer que l'attente du port NE SUFFIT PAS — la vraie
// waitForPort résout contre un orphelin, si bien que seul le fichier PID de
// NOTRE sshd distingue le nôtre de l'ancien ; (2) exiger que le harnais porte la
// garde (onPrepare asynchrone, écouteur `exit` sur sshd, attente du port,
// contrôle existsSync(sshd.pid), levée citant la fin de sshd.log ; et, côté
// pilote, refus de démarrer si 4444 répond déjà). Contre l'ancien harnais,
// onPrepare est synchrone et ces assertions échouent.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:net";
import { readFileSync, existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";

const ici = dirname(fileURLToPath(import.meta.url));
const racine = resolve(ici, "../..");
const source = readFileSync(resolve(racine, "e2e/wdio.conf.js"), "utf8");

// On importe la VRAIE waitForPort du harnais (pas une copie) : c'est elle que
// onPrepare emploie, donc c'est son comportement qu'on met à l'épreuve.
const { waitForPort } = await import(resolve(racine, "e2e/specs/helpers.js"));

test("attendre le port ne suffit pas : waitForPort résout contre un orphelin", async () => {
  // Un serveur qui n'accepte applicativement personne (maxConnections = 0) tient
  // pourtant le port : le noyau termine la poignée de main via le backlog. C'est
  // exactement ce que fait un sshd orphelin vis-à-vis d'une sonde TCP — il
  // « répond » sur 2223 sans être le nôtre. waitForPort résout donc, ce qui
  // prouve qu'elle ne peut pas, seule, détecter que notre sshd est mort.
  const orphelin = createServer();
  orphelin.maxConnections = 0;
  await new Promise((r) => orphelin.listen(0, "127.0.0.1", r));
  const port = orphelin.address().port;
  try {
    await waitForPort(port, 5000); // résout : le port répond
    // Le juge, c'est le fichier PID de NOTRE sshd dans NOTRE bac à sable, que
    // l'orphelin (autre bac) n'a pas écrit : un sshDir neuf ne le contient pas.
    const sshDir = mkdtempSync(join(tmpdir(), "avash-sshd-garde-"));
    assert.equal(existsSync(join(sshDir, "sshd.pid")), false,
      "l'absence du fichier PID doit rester le signe que ce n'est pas notre sshd qui écoute");
  } finally {
    orphelin.close();
  }
});

test("onPrepare est asynchrone (il doit pouvoir attendre le port)", async () => {
  // L'ancien harnais avait `onPrepare: () =>` : il ne pouvait pas attendre le
  // port ni lever au vu du PID. La garde exige un hook asynchrone.
  const { config } = await import(resolve(racine, "e2e/wdio.conf.js"));
  assert.equal(config.onPrepare.constructor.name, "AsyncFunction",
    "onPrepare doit être asynchrone pour attendre le port du sshd avant la suite");
});

test("startSshd est surveillé et le port est attendu au démarrage", () => {
  assert.match(source, /sshd\.once\("exit"/,
    "onPrepare doit attacher un écouteur `exit` au sshd (sa sortie renseigne le message d'échec)");
  assert.match(source, /waitForPort\(SSH_PORT\)/,
    "onPrepare doit attendre que le port du sshd réponde");
});

test("la présence de notre fichier PID prouve que c'est notre sshd qui écoute", () => {
  assert.match(source, /existsSync\(join\(sshDir, "sshd\.pid"\)\)/,
    "onPrepare doit exiger le fichier PID de notre sshd (écrit après un bind réussi)");
});

test("l'échec du sshd lève un message qui nomme le port occupé et cite sshd.log", () => {
  assert.match(source, /d[ée]j[àa] occup[ée]/,
    "le message d'échec doit suggérer que le port est déjà occupé par un orphelin");
  assert.match(source, /sshd\.log/,
    "le message d'échec doit joindre la fin de sshd.log (seule trace de la mort silencieuse)");
});

test("onPrepare refuse de démarrer si un tauri-driver orphelin tient déjà 4444", () => {
  // Symétrique du sshd : spawner un pilote neuf quand 4444 répond déjà le ferait
  // mourir sur bind(4444), laissant l'orphelin d'un ancien bac piloter la suite.
  assert.match(source, /if \(await pilotePret\(\)\)/,
    "onPrepare doit vérifier que 4444 est libre avant de lancer tauri-driver");
  assert.match(source, /4444 d[ée]j[àa] occup[ée]/,
    "onPrepare doit lever un message clair quand un tauri-driver orphelin répond déjà sur 4444");
});
