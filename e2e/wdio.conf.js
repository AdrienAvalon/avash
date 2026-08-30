// Harnais E2E : pilote la VRAIE app compilée (WebKitGTK) via tauri-driver.
// Seul niveau qui attrape les bugs du runtime réel (ex. window.prompt() inopérant
// sous WebKitGTK) et les flux utilisateur complets.
//
// Isolation : l'app tourne avec un HOME et un XDG_CONFIG_HOME temporaires — aucun
// hôte réel, registre de dossiers vierge, et surtout ZÉRO effet sur la vraie
// config de l'utilisateur (~/.ssh/config, ~/.config/avash).
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

let tauriDriver;
const sandbox = mkdtempSync(join(tmpdir(), "avash-e2e-"));

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
    tauriDriver = spawn("tauri-driver", [], {
      stdio: [null, process.stdout, process.stderr],
      env: { ...process.env, HOME: sandbox, XDG_CONFIG_HOME: join(sandbox, ".config") },
    });
  },
  onComplete: () => {
    if (tauriDriver) tauriDriver.kill();
  },
};
