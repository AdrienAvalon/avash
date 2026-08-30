// Harnais E2E : pilote la VRAIE app compilée (WebKitGTK) via tauri-driver.
// Seul niveau qui attrape les bugs du runtime réel (ex. window.prompt() inopérant
// sous WebKitGTK) et les flux utilisateur complets.
//
// Bac à sable : HOME + XDG_CONFIG_HOME temporaires, pré-remplis d'une config SSH
// de test (hôtes déterministes) — aucun effet sur la vraie config de l'utilisateur.
// Un serveur RDP de test local est démarré pour les scénarios RDP.
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

let tauriDriver;
let rdpServer;
const sandbox = mkdtempSync(join(tmpdir(), "avash-e2e-"));
export const RDP_PORT = 33899;

function seedSandbox() {
  // Config SSH de test : deux hôtes, dont un déjà rangé dans un dossier.
  const ssh = join(sandbox, ".ssh");
  mkdirSync(ssh, { recursive: true, mode: 0o700 });
  writeFileSync(
    join(ssh, "config"),
    [
      "Host web-1",
      "    HostName 10.0.0.1",
      "    User deploy",
      "    #Folder: prod",
      "",
      "Host db-1",
      "    HostName 10.0.0.2",
      "    User admin",
      "",
    ].join("\n"),
    { mode: 0o600 },
  );
}

export const config = {
  runner: "local",
  specs: ["./specs/**/*.spec.js"],
  maxInstances: 1,
  capabilities: [
    {
      "tauri:options": { application: "../target/release/avash-ui" },
      "wdio:maxInstances": 1,
    },
  ],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  mochaOpts: { ui: "bdd", timeout: 60000 },
  onPrepare: () => {
    seedSandbox();
    // Serveur RDP de test (mêmes identifiants que les specs : test/test).
    const srvDir = resolve("../test-rdp-server");
    rdpServer = spawn(
      "./target/release/test-rdp-server",
      ["--bind-addr", `127.0.0.1:${RDP_PORT}`, "--cert", "cert.pem", "--key", "key.pem",
       "--user", "test", "--pass", "test", "--sec", "hybrid"],
      { cwd: srvDir, stdio: "ignore" },
    );
    tauriDriver = spawn("tauri-driver", [], {
      stdio: [null, process.stdout, process.stderr],
      env: { ...process.env, HOME: sandbox, XDG_CONFIG_HOME: join(sandbox, ".config") },
    });
  },
  onComplete: () => {
    if (tauriDriver) tauriDriver.kill();
    if (rdpServer) rdpServer.kill();
  },
};
