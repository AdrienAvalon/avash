// Harnais E2E : pilote la VRAIE application compilée via WebdriverIO. Seul
// niveau qui attrape les bugs du runtime réel (ex. confirm() / prompt()
// inopérants sous WebKitGTK) et les flux utilisateur complets.
//
// Deux chemins vers l'application. Sous Linux, tauri-driver et WebKitWebDriver
// lancent l'application à chaque session. Sous Windows (et sur demande,
// E2E_EMBARQUE=1, partout), c'est le harnais qui lance l'application, compilée
// avec la fonctionnalité `webdriver` : elle embarque alors un serveur WebDriver
// (tauri-plugin-wdio-webdriver, port 4445) — Edge WebDriver ne sait plus lancer
// une application WebView2 depuis sa version 133 (« DevToolsActivePort file
// doesn't exist »), et macOS n'a aucun pilote. Une application par fichier de
// scénarios dans les deux cas : l'isolation ne dépend pas du chemin.
//
// Bac à sable : HOME + XDG_CONFIG_HOME temporaires, pré-remplis d'une config SSH
// de test — aucun effet sur la vraie config. Deux serveurs locaux sont démarrés
// pour les scénarios de bout en bout : un serveur RDP de test et un sshd dédié
// (non-root, clé, port 2223) auquel l'app se connecte réellement.
import { spawn, execSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, chmodSync, rmSync } from "node:fs";
import { tmpdir, userInfo } from "node:os";
import { join } from "node:path";

let tauriDriver;
let sshd;
let appEmbarquee;
// Le lanceur crée le bac à sable et le publie dans l'environnement : les
// processus de travail, forkés ensuite, retrouvent le MÊME chemin — sans quoi
// chacun en créerait un différent et ne pourrait pas remettre à zéro celui que
// l'application utilise réellement.
const sandbox = process.env.AVASH_E2E_SANDBOX ?? mkdtempSync(join(tmpdir(), "avash-e2e-"));
const sshDir = join(sandbox, "sshtest");
export const RDP_PORT = 33899;
export const SSH_PORT = 2223;
// Serveurs locaux (RDP + sshd) : actifs partout, intégration continue comprise,
// qui construit le serveur RDP de test et génère son certificat. `E2E_NO_RDP=1`
// reste disponible pour une machine sans sshd ni serveur de test.
// Sous Windows, ni sshd de harnais ni serveur RDP de test : seuls les scénarios
// sans serveur local tournent — c'est déjà ce que la CI Linux ne voyait pas.
export const WINDOWS = process.platform === "win32";
export const LOCAL_SERVERS = !process.env.E2E_NO_RDP && !WINDOWS;
// Serveur WebDriver embarqué dans l'application (cf. en-tête) : d'office sous
// Windows, sur demande ailleurs.
export const EMBARQUE = WINDOWS || !!process.env.E2E_EMBARQUE;
const PORT_EMBARQUE = 4445;
const APP = join(import.meta.dirname, "..", "target", "release", WINDOWS ? "avash-ui.exe" : "avash-ui");

// L'environnement de l'application pilotée, quel que soit le chemin qui la
// lance. AVASH_HOME en plus de HOME/XDG_CONFIG_HOME : sous Windows, l'API qui
// donne le répertoire de configuration interroge le shell et ignore les deux
// autres. Sans cette variable, la suite écrirait dans les fichiers RÉELS de
// l'utilisateur — config SSH et fichier de confiance RDP.
const ENV_APP = {
  ...process.env,
  HOME: sandbox,
  AVASH_HOME: sandbox,
  // La langue suit la locale au premier lancement : les scénarios affirment
  // des textes français, la webview doit se croire en France quelle que soit
  // la machine (la locale n'a pas à être installée, WebKit lit ces variables
  // telles quelles).
  LANGUAGE: "fr_FR:fr",
  LANG: "fr_FR.UTF-8",
  LC_ALL: "fr_FR.UTF-8",
  // … et quand la locale n'est pas installée sur la machine (chaîne
  // d'intégration), la webview démarre quand même en anglais : le cœur impose
  // alors la langue avant le premier script (AVASH_LANGUE).
  AVASH_LANGUE: "fr",
  XDG_CONFIG_HOME: join(sandbox, ".config"),
  // Surtout pas WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS ici : l'application
  // retire toute valeur héritée, et n'en a pas besoin — le serveur embarqué
  // n'ouvre aucun port de débogage Chromium.
};

/** Lance l'application compilée avec son serveur WebDriver, et attend qu'il réponde. */
async function lancerAppEmbarquee() {
  const app = spawn(APP, [], {
    env: { ...ENV_APP, TAURI_WEBDRIVER_PORT: String(PORT_EMBARQUE) },
    stdio: ["ignore", "inherit", "inherit"],
  });
  let sortie = null;
  app.on("exit", (code, signal) => { sortie = { code, signal }; });
  const echeance = Date.now() + 60000;
  while (Date.now() < echeance) {
    if (sortie) throw new Error(`l'application s'est arrêtée avant d'être pilotable (code ${sortie.code}, signal ${sortie.signal})`);
    try {
      const r = await fetch(`http://127.0.0.1:${PORT_EMBARQUE}/status`, { signal: AbortSignal.timeout(2000) });
      if (r.ok && (await r.json()).value?.ready) return app;
    } catch { /* pas encore à l'écoute */ }
    await new Promise((res) => setTimeout(res, 250));
  }
  app.kill();
  throw new Error("le serveur WebDriver embarqué n'a jamais répondu (60 s)");
}

/** Arrête l'application lancée par le harnais, et attend sa sortie. */
function arreterAppEmbarquee(app) {
  if (!app || app.exitCode !== null) return Promise.resolve();
  return new Promise((res) => {
    const force = setTimeout(() => { try { app.kill("SIGKILL"); } catch { /* déjà partie */ } }, 5000);
    app.once("exit", () => { clearTimeout(force); res(); });
    app.kill();
  });
}

// Les hôtes réellement semés, selon que les serveurs locaux tournent ou non.
// Exporté pour que les specs raisonnent sur le semage plutôt que de le
// réénoncer : `isolation.spec.js` affirmait « db-1, test-ssh, web-1 » alors que
// la CI ne sème pas `test-ssh` — la garde d'isolation ne pouvait qu'y échouer.
// Chemin de la clé cliente semée : les scénarios qui remplissent le formulaire
// « Connexion directe » en ont besoin, et le déduire chez eux les rendrait faux
// le jour où le harnais change d'emplacement.
export const CLE_CLIENTE = join(sandbox, ".ssh", "test_client");

export const HOTES_SEMES = LOCAL_SERVERS ? ["db-1", "test-ssh", "web-1"] : ["db-1", "web-1"];

function seedSandbox() {
  const ssh = join(sandbox, ".ssh");
  mkdirSync(ssh, { recursive: true, mode: 0o700 });
  const lines = [
    "Host web-1", "    HostName 10.0.0.1", "    User deploy", "    #Folder: prod", "",
    "Host db-1", "    HostName 10.0.0.2", "    User admin", "",
  ];
  if (LOCAL_SERVERS) {
    // Hôte réellement joignable, servi par le sshd local ci-dessous.
    lines.push(
      "Host test-ssh", "    HostName 127.0.0.1", `    Port ${SSH_PORT}`,
      `    User ${userInfo().username}`, `    IdentityFile ${join(ssh, "test_client")}`, "");
  }
  writeFileSync(join(ssh, "config"), lines.join("\n"), { mode: 0o600 });

  // Sessions PuTTY, telles que l'outil les range sous Unix : de quoi exercer
  // l'import sans dépendre d'un PuTTY installé. « Default Settings » n'est pas
  // une session et doit être ignorée ; la session série n'est pas du SSH.
  const putty = join(sandbox, ".putty", "sessions");
  rmSync(putty, { recursive: true, force: true });
  mkdirSync(putty, { recursive: true, mode: 0o700 });
  writeFileSync(join(putty, "Default%20Settings"), "HostName=\nProtocol=ssh\n");
  writeFileSync(join(putty, "prod%20web"), "HostName=10.0.0.7\nPortNumber=2222\nUserName=adrien\nProtocol=ssh\nPublicKeyFile=/home/a/cle.ppk\n");
  writeFileSync(join(putty, "console%20serie"), "Protocol=serial\nSerialLine=/dev/ttyUSB0\n");

  // État applicatif (dossiers, bureaux RDP, snippets, tunnels) : on le supprime
  // pour que chaque fichier de tests reparte du même point. Le dossier .ssh est
  // conservé : il porte la clé cliente du sshd, générée une seule fois.
  rmSync(join(sandbox, ".config", "avash"), { recursive: true, force: true });
}

function startSshd() {
  mkdirSync(sshDir, { recursive: true, mode: 0o700 });
  const ssh = join(sandbox, ".ssh");
  const host = join(sshDir, "hostkey");
  const client = join(ssh, "test_client");
  const authKeys = join(sshDir, "authorized_keys");
  const cfg = join(sshDir, "sshd_config");
  execSync(`ssh-keygen -t ed25519 -f "${host}" -N "" -q`);
  execSync(`ssh-keygen -t ed25519 -f "${client}" -N "" -q`);
  execSync(`cp "${client}.pub" "${authKeys}"`);
  chmodSync(authKeys, 0o600); chmodSync(host, 0o600);
  writeFileSync(cfg, [
    `Port ${SSH_PORT}`, "ListenAddress 127.0.0.1", `HostKey ${host}`,
    `PidFile ${join(sshDir, "sshd.pid")}`, `AuthorizedKeysFile ${authKeys}`,
    "UsePAM no", "PasswordAuthentication no", "PubkeyAuthentication yes",
    "StrictModes no", "Subsystem sftp internal-sftp", "",
  ].join("\n"));
  // `sshd` vit dans /usr/sbin, qui n'est pas toujours sur le PATH d'un
  // exécuteur d'intégration continue : on le cherche là aussi.
  const sshdBin = execSync(
    "command -v sshd || ls /usr/sbin/sshd 2>/dev/null || echo /usr/bin/sshd",
  ).toString().trim();
  // -D : reste au premier plan pour qu'on tienne le processus et qu'on le tue à la fin.
  return spawn(sshdBin, ["-D", "-f", cfg, "-E", join(sshDir, "sshd.log")], { stdio: "ignore" });
}

export const config = {
  runner: "local",
  specs: ["./specs/**/*.spec.js"],
  // On DÉSIGNE ce qui exige un serveur local, plutôt que d'énumérer ce qui n'en
  // exige pas : la liste énumérative prenait du retard à chaque spec ajoutée —
  // cinq scénarios pourtant sans serveur ne tournaient plus qu'en local. Une
  // nouvelle spec sans serveur tourne désormais en CI d'office.
  exclude: LOCAL_SERVERS
    ? []
    : [
        "./specs/ssh.spec.js", "./specs/sftp.spec.js",
        "./specs/rdp.spec.js", "./specs/rdp-reconnect.spec.js",
        "./specs/rdp-clipboard.spec.js",
        "./specs/onglets-mixtes.spec.js", "./specs/enregistrer-et-connecter.spec.js",
        "./specs/enregistrement.spec.js", "./specs/sante.spec.js",
      ],
  maxInstances: 1,
  // Régression visuelle : captures comparées pixel à pixel à des références.
  // Le service n'est chargé qu'à la demande (VISUEL=1) : branché en
  // permanence, il multipliait par vingt la durée de chaque fichier de
  // scénarios. La chaîne lance le scénario visuel dans un passage à part.
  // Les références du dépôt sont celles de la chaîne (ubuntu-latest) : les
  // polices d'une autre machine ne rendent pas pareil, donc en local les
  // captures vont dans un dossier ignoré par git.
  services: !process.env.VISUEL ? [] : [
    ["visual", {
      baselineFolder: join(import.meta.dirname, process.env.CI ? "visuel/reference" : ".tmp/visuel-local/reference"),
      screenshotPath: join(import.meta.dirname, ".tmp/visuel"),
      formatImageName: "{tag}",
      autoSaveBaseline: true,
      savePerInstance: false,
      blockOutStatusBar: false,
      blockOutToolBar: false,
    }],
  ],
  capabilities: [
    { "tauri:options": { application: WINDOWS ? "../target/release/avash-ui.exe" : "../target/release/avash-ui" }, "wdio:maxInstances": 1 },
  ],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  hostname: "127.0.0.1",
  port: EMBARQUE ? PORT_EMBARQUE : 4444,
  path: "/",
  mochaOpts: { ui: "bdd", timeout: 60000 },
  // Le défaut de WebdriverIO est de 3 s. C'est court pour une application
  // native qui vient de démarrer sur une machine occupée — et c'est la valeur
  // qu'utilise toute attente écrite sans échéance explicite.
  waitforTimeout: 10000,
  onPrepare: () => {
    process.env.AVASH_E2E_SANDBOX = sandbox; // hérité par les workers
    seedSandbox(); // crée ~/.ssh + config (référence la clé cliente)
    if (LOCAL_SERVERS) sshd = startSshd(); // génère cette clé dans ~/.ssh, démarre le sshd
    // Les serveurs RDP de test sont démarrés PAR CHAQUE spec RDP (serveur dédié,
    // cf. rdp.spec/rdp-reconnect.spec) : pas de serveur partagé à coupler.
    // Avec le serveur embarqué, c'est `beforeSession` qui lance l'application.
    if (EMBARQUE) return;
    tauriDriver = spawn("tauri-driver", [], {
      stdio: [null, process.stdout, process.stderr],
      env: ENV_APP,
    });
  },
  // Avant CHAQUE fichier de spécifications : on remet le bac à sable dans son
  // état semé. L'application démarre ensuite et lit un état déterministe, quel
  // que soit ce qu'ont fait les fichiers précédents (spécification isolation).
  beforeSession: async () => {
    seedSandbox();
    if (EMBARQUE) appEmbarquee = await lancerAppEmbarquee();
  },
  afterSession: async () => {
    if (EMBARQUE) { await arreterAppEmbarquee(appEmbarquee); appEmbarquee = null; }
  },
  // ... et on attend qu'elle soit RÉELLEMENT prête avant le premier geste.
  //
  // Chaque fichier relance l'application ; les scénarios enchaînaient aussitôt
  // sur un clic. Entre le démarrage de la fenêtre et le premier rendu, il y a
  // le chargement du front puis un aller-retour vers le cœur pour lire la
  // configuration : agir avant que cela n'aboutisse frappe un DOM à moitié
  // câblé. C'est la cause d'une famille entière d'échecs intermittents, qui ne
  // se manifestaient que sur une machine occupée.
  //
  // `#host-list` reçoit toujours au moins un enfant à la fin du rendu — une
  // ligne d'hôte, ou le message d'accueil quand la configuration est vide.
  before: async () => {
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const l = document.getElementById("host-list");
          return document.readyState === "complete" && !!l && l.children.length > 0;
        }),
      { timeout: 30000, timeoutMsg: "l'application n'a jamais fini de démarrer" },
    );
  },
  onComplete: () => {
    if (tauriDriver) tauriDriver.kill();
    if (sshd) sshd.kill();
  },
};
