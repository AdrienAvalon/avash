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
import { mkdtempSync, mkdirSync, writeFileSync, appendFileSync, chmodSync, rmSync, copyFileSync, existsSync, readFileSync } from "node:fs";
import { tmpdir, userInfo } from "node:os";
import { join } from "node:path";
// Attente d'un port, partagée avec les specs (une seule implémentation) : sert à
// vérifier au démarrage que le sshd de test écoute vraiment (cf. onPrepare).
import { waitForPort } from "./specs/helpers.js";

let tauriDriver;
let sshd;
let appEmbarquee;
// Le lanceur crée le bac à sable et le publie dans l'environnement : les
// processus de travail, forkés ensuite, retrouvent le MÊME chemin — sans quoi
// chacun en créerait un différent et ne pourrait pas remettre à zéro celui que
// l'application utilise réellement.
// Un bac à sable fourni de l'extérieur (AVASH_E2E_SANDBOX posé avant le run)
// appartient à l'appelant : onComplete ne le supprime pas. Celui qu'on crée
// nous-mêmes, si.
const SANDBOX_FOURNI = !!process.env.AVASH_E2E_SANDBOX;
const sandbox = process.env.AVASH_E2E_SANDBOX ?? mkdtempSync(join(tmpdir(), "avash-e2e-"));
const sshDir = join(sandbox, "sshtest");
export const WINDOWS = process.platform === "win32";
export const RDP_PORT = 33899;
// Sous Windows, le sshd est le service OpenSSH Server du système (voir
// preparerSshdWindows) : il écoute sur le port 22, on ne choisit pas.
export const SSH_PORT = WINDOWS ? 22 : 2223;
// Serveurs locaux (RDP, VNC, sshd) : actifs partout, intégration continue
// comprise, qui construit les serveurs de test et génère le certificat RDP.
// `E2E_NO_RDP=1` reste disponible pour une machine sans sshd ni serveur de
// test. Sous Windows, ils ont tourné pour la première fois le 05/09/2026 :
// jusque-là seuls les scénarios sans serveur y passaient, et les quatre bogues
// Windows de l'été avaient tous été trouvés en publiant.
export const LOCAL_SERVERS = !process.env.E2E_NO_RDP;
// Un chemin tel que le sshd et l'application le lisent : sous Windows, les
// barres obliques évitent qu'un `\t` ou un `\n` de C:\temp ne soit pris pour
// une séquence d'échappement par un parseur de configuration.
const chemin = (p) => (WINDOWS ? p.replace(/\\/g, "/") : p);
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
export const ENV_APP = {
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
  // Trouvé par l'audit du 7 septembre 2026 : la webview range plusieurs
  // réglages dans son stockage web (avash.langue, qui PRIME sur AVASH_LANGUE
  // dans lireLangue ; avash.rdp.clipboard ; avash.rdp.son ; avash.sante ;
  // thème ; largeurs de panneaux ; dossiers repliés ; cache des logos d'OS).
  // Sous Linux, Tauri résout ce répertoire par BaseDirectory::LocalData, donc
  // XDG_DATA_HOME sinon $HOME/.local/share. Sans redirection de ces trois
  // variables dans le bac à sable, un poste où XDG_DATA_HOME est exporté ferait
  // écrire la suite dans les données RÉELLES de l'utilisateur (même identifiant
  // dev.avash.app que l'application installée).
  XDG_DATA_HOME: join(sandbox, ".local", "share"),
  XDG_CACHE_HOME: join(sandbox, ".cache"),
  XDG_STATE_HOME: join(sandbox, ".local", "state"),
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

export function seedSandbox() {
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
      `    User ${userInfo().username}`, `    IdentityFile ${chemin(join(ssh, "test_client"))}`, "");
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
  // Trouvé par l'audit du 7 septembre 2026 : le stockage web de la webview ne
  // vit PAS sous .config/avash mais sous les répertoires XDG de données, de
  // cache et d'état (cf. ENV_APP). Ne pas les effacer laissait persister
  // avash.sante d'un fichier à l'autre (voyants parasites, un VISUEL=1 local
  // qui diffère des références), et un échec de langue.spec après « Switch to
  // English » démarrait tous les fichiers suivants en anglais — avash.langue
  // primant sur AVASH_LANGUE dans lireLangue — sans lien avec leur objet.
  rmSync(join(sandbox, ".local", "share"), { recursive: true, force: true });
  rmSync(join(sandbox, ".local", "state"), { recursive: true, force: true });
  rmSync(join(sandbox, ".cache"), { recursive: true, force: true });
}

function startSshd() {
  mkdirSync(sshDir, { recursive: true, mode: 0o700 });
  const ssh = join(sandbox, ".ssh");
  const host = join(sshDir, "hostkey");
  const client = join(ssh, "test_client");
  const authKeys = join(sshDir, "authorized_keys");
  const cfg = join(sshDir, "sshd_config");
  execSync(`ssh-keygen -t ed25519 -f "${client}" -N "" -q`);
  if (WINDOWS) return preparerSshdWindows(`${client}.pub`);
  execSync(`ssh-keygen -t ed25519 -f "${host}" -N "" -q`);
  copyFileSync(`${client}.pub`, authKeys);
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

// PowerShell non interactif, guillemets échappés pour `-Command`. Hoisté au
// module : la restauration de fin de suite (restaurerSshdWindows) en a besoin
// autant que la préparation.
const powershell = (script) => execSync(`powershell -NoProfile -NonInteractive -Command "${script.replace(/"/g, '\\"')}"`, { stdio: "pipe" }).toString();

// État de restauration du sshd SYSTÈME : renseigné par preparerSshdWindows,
// consommé par restaurerSshdWindows dans onComplete. onPrepare et onComplete
// tournent dans le même processus lanceur, donc ces variables de module font le
// lien (les workers, eux, ne les partagent pas).
let sshdWindowsARestaurer = false;
let cheminAutorisees = null;
let sauvegardeAutorisees = null; // .bak si le fichier préexistait ; null si créé par nous

// Sous Windows, on ne lance pas un sshd à nous : l'authentification par clé
// d'OpenSSH pour Windows passe par une ouverture de session S4U que seul le
// compte SYSTEM peut faire, donc par le service « sshd » (OpenSSH Server,
// capacité facultative installée par la chaîne). Le harnais lui confie la clé
// cliente dans administrators_authorized_keys, le fichier que sa configuration
// par défaut lit pour les administrateurs (l'exécuteur en est un), avec les
// droits qu'il exige (SYSTEM et Administrateurs seulement), puis le redémarre.
// Le shell de connexion (bash de Git for Windows) est posé dans le registre
// par la chaîne, avant.
export function preparerSshdWindows(clePublique) {
  // Trouvé par l'audit du 7 septembre 2026 : ce chemin touche le sshd du
  // SYSTÈME (port 22 réel), pas un serveur de test isolé. Rien ne l'exigeait en
  // CI : un simple `npm test` dans un terminal élevé écrasait les clés d'admin
  // de la machine et laissait la clé de test autorisée après la suite. On
  // l'exige désormais explicitement, AVANT tout démarrage de service — ce qui
  // change aussi l'échec cryptique « Start-Service » (terminal non élevé) en
  // message clair.
  if (!process.env.CI && !process.env.E2E_SSHD_SYSTEME) {
    throw new Error("le sshd système n'est modifié qu'en CI ; poser E2E_SSHD_SYSTEME=1 en connaissance de cause, ou E2E_NO_RDP=1 pour sauter les serveurs locaux");
  }
  const programData = process.env.ProgramData ?? "C:\\ProgramData";
  const autorisees = join(programData, "ssh", "administrators_authorized_keys");
  // Un premier démarrage crée C:\ProgramData\ssh (clés d'hôte, sshd_config).
  powershell("Start-Service sshd");
  // Sauvegarde AVANT toute écriture : le fichier peut porter plusieurs clés
  // d'admin légitimes. On n'AJOUTE que la ligne de la clé de test ; onComplete
  // restaure l'état d'origine (ou retire le fichier qu'on a créé).
  cheminAutorisees = autorisees;
  if (existsSync(autorisees)) {
    sauvegardeAutorisees = `${autorisees}.avash-e2e.bak`;
    copyFileSync(autorisees, sauvegardeAutorisees);
    appendFileSync(autorisees, `${copierCle(clePublique)}\n`);
  } else {
    sauvegardeAutorisees = null;
    writeFileSync(autorisees, `${copierCle(clePublique)}\n`);
  }
  sshdWindowsARestaurer = true;
  powershell(`icacls '${autorisees}' /inheritance:r /grant 'SYSTEM:F' /grant 'Administrators:F' /grant 'BUILTIN\\Administrators:F'`);
  powershell("Restart-Service sshd");
  return null;
}

// Rend au sshd SYSTÈME l'état d'avant la suite : restaure le fichier de clés
// d'admin sauvegardé (ou retire celui qu'on a créé), réapplique ses ACL puis
// redémarre le service. Sans quoi la clé de test resterait autorisée sur le
// port 22 réel de la machine (constat de l'audit du 7 septembre 2026).
function restaurerSshdWindows() {
  if (!sshdWindowsARestaurer) return;
  if (sauvegardeAutorisees && existsSync(sauvegardeAutorisees)) {
    copyFileSync(sauvegardeAutorisees, cheminAutorisees);
    rmSync(sauvegardeAutorisees, { force: true });
  } else if (cheminAutorisees) {
    rmSync(cheminAutorisees, { force: true });
  }
  if (cheminAutorisees && existsSync(cheminAutorisees)) {
    powershell(`icacls '${cheminAutorisees}' /inheritance:r /grant 'SYSTEM:F' /grant 'Administrators:F' /grant 'BUILTIN\\Administrators:F'`);
  }
  powershell("Restart-Service sshd");
  sshdWindowsARestaurer = false;
}

function copierCle(fichier) {
  return execSync(WINDOWS ? `type "${fichier}"` : `cat "${fichier}"`).toString().trim();
}

// Le pilote natif (WebKitWebDriver, lancé par tauri-driver) est mort une fois
// en pleine suite : chaîne GitLab #3382 du 2026-09-04, « connection closed »
// puis « Connection refused » pour tout ce qui suivait, deux fichiers perdus
// sur vingt-six, alors que les trois exécutions précédentes en passaient
// vingt-six. Rien ne le relançait. Avant chaque fichier, on vérifie qu'il
// répond ; sinon on relance tauri-driver, qui relance le natif, sur un port
// natif neuf au cas où l'ancien processus traînerait encore sur le sien.
//
// Trouvé par l'audit du 7 septembre 2026 : cette vérification vivait dans
// `beforeSession`, hook qui tourne dans le PROCESSUS DE TRAVAIL, alors que la
// poignée `tauriDriver` et le compteur `relances` sont posés par onPrepare dans
// le LANCEUR. Dans le travailleur, `tauriDriver` valait undefined (le kill ne
// tuait rien) et `relances` repartait de zéro à chaque fichier ; pire, le
// tauri-driver relancé appartenait alors au travailleur, donc onComplete (dans
// le lanceur) ne le tuait jamais et un pilote orphelin gardait le port 4444,
// empoisonnant l'exécution suivante. La relance vit désormais dans le hook de
// lanceur `onWorkerStart`, où la poignée existe et où `relances` persiste.
const PORT_NATIF = 4445;
let relances = 0;

function lancerTauriDriver() {
  const args = relances ? ["--native-port", String(PORT_NATIF + relances)] : [];
  tauriDriver = spawn("tauri-driver", args, {
    stdio: [null, process.stdout, process.stderr],
    env: ENV_APP,
  });
}

async function pilotePret() {
  try {
    const r = await fetch("http://127.0.0.1:4444/status", { signal: AbortSignal.timeout(3000) });
    return r.ok;
  } catch {
    return false;
  }
}

// Tue un tauri-driver et ATTEND sa sortie. tauri-driver ne libère le port 4444
// qu'en s'arrêtant (sa boucle `accept` tourne tant qu'il vit) et ne tue son
// natif que sur SIGTERM (server.rs) : relancer sans attendre ferait échouer le
// nouveau pilote sur `TcpListener::bind(4444)` (« can not listen to address »),
// qui sortirait en laissant son natif orphelin.
function arreterTauriDriver(proc) {
  if (!proc || proc.exitCode !== null) return Promise.resolve();
  return new Promise((res) => {
    const force = setTimeout(() => { try { proc.kill("SIGKILL"); } catch { /* déjà parti */ } }, 5000);
    proc.once("exit", () => { clearTimeout(force); res(); });
    proc.kill();
  });
}

export async function relancerPiloteSiMort() {
  // Petite tolérance avant de conclure à la mort : onWorkerStart tourne tôt
  // après onPrepare, et sur une machine chargée tauri-driver peut n'avoir pas
  // encore ouvert 4444 — sans quoi on relancerait le pilote fraîchement lancé.
  for (let i = 0; i < 10; i++) {
    if (await pilotePret()) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  relances += 1;
  console.warn(`e2e : le pilote WebDriver ne répond plus, relance de tauri-driver (${relances})`);
  await arreterTauriDriver(tauriDriver);
  lancerTauriDriver();
  for (let i = 0; i < 100; i++) {
    if (await pilotePret()) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error("e2e : tauri-driver ne répond pas vingt secondes après sa relance");
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
        "./specs/rdp-clipboard.spec.js", "./specs/rdp-fichiers.spec.js", "./specs/rdp-audio.spec.js", "./specs/rdp-lecteur.spec.js", "./specs/vnc.spec.js", "./specs/vnc-tls.spec.js",
        "./specs/onglets-mixtes.spec.js", "./specs/enregistrer-et-connecter.spec.js",
        "./specs/enregistrement.spec.js", "./specs/sante.spec.js",
        "./specs/restauration.spec.js", "./specs/vue-partagee.spec.js", "./specs/serie.spec.js",
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
      // Trouvé par l'audit du 7 septembre 2026 : `autoSaveBaseline: true` en dur
      // rendait l'étape « régression visuelle » incapable de rougir en CI. Un
      // tag ajouté ou renommé dans visuel.spec.js sans commiter son PNG voyait
      // sa référence refabriquée à chaque run (exécuteur neuf) et checkScreen
      // rendre 0 : vert, sans jamais rien comparer. En CI la référence absente
      // lève désormais « Baseline image not found » et l'étape rougit ;
      // VISUEL_INIT=1 rouvre l'auto-sauvegarde pour amorcer volontairement
      // (workflow_dispatch, ou en local), et hors CI les références locales du
      // dossier ignoré .tmp/visuel-local se créent toujours au premier passage.
      autoSaveBaseline: !process.env.CI || !!process.env.VISUEL_INIT,
      // Toujours écrire la capture réelle sur disque : sans cela, quand
      // autoSaveBaseline est off et la référence manque, l'image réelle n'est
      // pas sauvée et l'artefact e2e/.tmp/visuel ne fournit pas au contributeur
      // le PNG à copier dans e2e/visuel/reference.
      alwaysSaveActualImage: true,
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
  onPrepare: async () => {
    process.env.AVASH_E2E_SANDBOX = sandbox; // hérité par les workers
    seedSandbox(); // crée ~/.ssh + config (référence la clé cliente)
    if (LOCAL_SERVERS) {
      sshd = startSshd(); // génère cette clé dans ~/.ssh, démarre le sshd
      // Trouvé par l'audit du 8 septembre 2026 : startSshd spawnait sshd avec
      // stdio ignoré et sans écouteur `exit`, et rien ici n'attendait le port ni
      // le PID. Un sshd orphelin d'un run précédent (lanceur tué sans onComplete
      // — SIGKILL, timeout CI local) tenant déjà le port 2223 faisait mourir
      // NOTRE sshd neuf sur le bind, en silence ; l'ancien, configuré avec
      // l'authorized_keys d'un AUTRE bac à sable, répondait à sa place et
      // refusait la clé cliente toute neuve — toute la suite SSH échouait
      // « jamais live » / « Permission denied » sans jamais nommer le port
      // occupé. On capte la sortie du processus, on attend le port, puis on
      // exige le fichier PID de notre sshd : sshd ne l'écrit qu'après un bind
      // réussi, donc sa présence prouve que c'est bien le nôtre qui écoute, son
      // absence trahit l'orphelin. (Sous Windows, startSshd pilote le service
      // système et rend null : la garde ne vaut que pour le sshd dédié.)
      if (sshd) {
        let sortieSshd = null;
        sshd.once("exit", (code) => { sortieSshd = code; });
        // Le port peut ne jamais répondre (bind échoué, aucun orphelin) : on
        // laisse le contrôle du PID ci-dessous lever avec le journal.
        await waitForPort(SSH_PORT).catch(() => {});
        if (!existsSync(join(sshDir, "sshd.pid"))) {
          const chemLog = join(sshDir, "sshd.log");
          const journal = existsSync(chemLog)
            ? readFileSync(chemLog, "utf8").split("\n").slice(-10).join("\n")
            : "(pas de sshd.log)";
          throw new Error(
            `le sshd de test n'a pas démarré (sortie ${sortieSshd}) ; port ${SSH_PORT} déjà occupé par un sshd orphelin d'un run précédent ?\n--- fin de sshd.log ---\n${journal}`,
          );
        }
      }
    }
    // Les serveurs RDP de test sont démarrés PAR CHAQUE spec RDP (serveur dédié,
    // cf. rdp.spec/rdp-reconnect.spec) : pas de serveur partagé à coupler.
    // Avec le serveur embarqué, c'est `beforeSession` qui lance l'application.
    if (EMBARQUE) return;
    // Symétrique du sshd : si 4444 répond déjà, un tauri-driver orphelin d'un run
    // précédent tient le port ; en spawner un neuf le ferait mourir sur
    // bind(4444) et l'orphelin, lié à l'ENV_APP d'un ancien bac à sable (HOME
    // supprimé), piloterait la suite à sa place. On lève plutôt que de servir
    // l'orphelin en silence.
    if (await pilotePret()) {
      throw new Error("port 4444 déjà occupé : un tauri-driver orphelin d'un run précédent répond ; le tuer avant de relancer la suite");
    }
    lancerTauriDriver();
  },
  // Avant CHAQUE processus de travail, dans le LANCEUR : on s'assure que le
  // pilote répond encore, sinon on le relance. Ce hook (contrairement à
  // beforeSession) tourne dans @wdio/cli, là où vivent la poignée `tauriDriver`
  // et le compteur `relances` — sans quoi la relance ne pouvait ni tuer l'ancien
  // pilote ni persister d'un fichier à l'autre (cf. commentaire de relancer).
  // Le chemin embarqué n'a pas de tauri-driver : rien à faire ici pour lui.
  onWorkerStart: async () => {
    if (EMBARQUE) return;
    await relancerPiloteSiMort();
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
    // Sous Windows, le sshd est le service SYSTÈME : on lui rend son fichier de
    // clés d'admin d'avant la suite, sans quoi la clé de test resterait
    // autorisée sur le port 22 réel de la machine.
    if (WINDOWS) restaurerSshdWindows();
    // Trouvé par l'audit du 7 septembre 2026 : onComplete tuait les serveurs
    // mais laissait le bac à sable (clé privée cliente, clé d'hôte, sshd.log,
    // config) dans /tmp. Sur un poste où /tmp est un tmpfs, chaque run en
    // gardait un — la clé privée du sshd de test restait lisible en mémoire
    // vive jusqu'au redémarrage, et les répertoires s'accumulaient. On le
    // supprime, sauf E2E_GARDER_SANDBOX pour inspecter l'état après un échec,
    // et sauf un bac fourni par l'appelant (qui en garde la propriété).
    if (!process.env.E2E_GARDER_SANDBOX && !SANDBOX_FOURNI) {
      rmSync(sandbox, { recursive: true, force: true });
    }
  },
};
