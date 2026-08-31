// Harnais E2E : pilote la VRAIE application compilée (WebKitGTK) via tauri-driver
// + WebdriverIO. Seul niveau qui attrape les bugs du runtime réel (ex. confirm() /
// prompt() inopérants sous WebKitGTK) et les flux utilisateur complets.
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
// Le lanceur crée le bac à sable et le publie dans l'environnement : les
// processus de travail, forkés ensuite, retrouvent le MÊME chemin — sans quoi
// chacun en créerait un différent et ne pourrait pas remettre à zéro celui que
// l'application utilise réellement.
const sandbox = process.env.AVASH_E2E_SANDBOX ?? mkdtempSync(join(tmpdir(), "avash-e2e-"));
const sshDir = join(sandbox, "sshtest");
export const RDP_PORT = 33899;
export const SSH_PORT = 2223;
// Serveurs locaux (RDP + sshd) désactivés en CI : le sshd non-root et les certs
// de test ne s'y prêtent pas. Les specs qui en dépendent sont retirées de la liste.
export const LOCAL_SERVERS = !process.env.E2E_NO_RDP;

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
  const sshdBin = execSync("command -v sshd || echo /usr/bin/sshd").toString().trim();
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
      ],
  maxInstances: 1,
  capabilities: [
    { "tauri:options": { application: "../target/release/avash-ui" }, "wdio:maxInstances": 1 },
  ],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  hostname: "127.0.0.1",
  port: 4444,
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
    tauriDriver = spawn("tauri-driver", [], {
      stdio: [null, process.stdout, process.stderr],
      env: { ...process.env, HOME: sandbox, XDG_CONFIG_HOME: join(sandbox, ".config") },
    });
  },
  // Avant CHAQUE fichier de spécifications : on remet le bac à sable dans son
  // état semé. L'application démarre ensuite et lit un état déterministe, quel
  // que soit ce qu'ont fait les fichiers précédents (spécification isolation).
  beforeSession: () => {
    seedSandbox();
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
