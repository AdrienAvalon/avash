// Captures d'écran du README (docs/captures/), prises sur la vraie
// application par le même harnais que la suite bout en bout : bac à sable
// semé, sshd local, et un bureau Windows du parc du mainteneur pour la vue
// RDP. `scripts/captures-readme.sh` lance ce fichier sous Xvfb.
//
// Ce n'est pas un test : rien n'est comparé, et ce fichier n'est pas dans la
// liste des spécifications de la suite.
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { config as base } from "./wdio.conf.js";

export const config = {
  ...base,
  specs: ["./captures/readme.spec.js"],
  beforeSession: async (...args) => {
    await base.beforeSession(...args);
    const sandbox = process.env.AVASH_E2E_SANDBOX;
    // Des hôtes plausibles en plus de ceux du bac à sable, pour que la barre
    // latérale ressemble à un parc. Adresses documentaires (RFC 5737, 1918).
    appendFileSync(
      join(sandbox, ".ssh", "config"),
      [
        "Host bastion", "    HostName 203.0.113.10", "    User ops", "    #Folder: prod", "",
        "Host api-1", "    HostName 10.0.0.3", "    User deploy", "    #Folder: prod", "",
        "Host build", "    HostName 198.51.100.7", "    User ci", "    #Folder: lab", "",
      ].join("\n") + "\n",
    );
    // Un bureau RDP dans la liste : le fichier des hôtes RDP de l'application.
    const conf = join(sandbox, ".config", "avash");
    mkdirSync(conf, { recursive: true, mode: 0o700 });
    writeFileSync(
      join(conf, "rdp.yaml"),
      [
        "- id: rdp-win-01",
        "  name: win-01",
        "  host: 10.0.0.20",
        "  port: 3389",
        "  user: 'LAB\\admin'",
        "  width: 1280",
        "  height: 800",
        "  folder: lab",
        "",
      ].join("\n"),
      { mode: 0o600 },
    );
  },
};
