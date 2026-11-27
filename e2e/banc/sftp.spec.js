// Banc S6 : débits SFTP, mesurés par l'exemple Rust banc_sftp (fonctions de
// production du cœur) contre le sshd du harnais (internal-sftp d'OpenSSH).
//
// Le chemin de l'exemple vient de BANC_SFTP_BIN (construit par
// scripts/banc.mjs). Avec BANC_LATENCE_MS (Linux), le lanceur a injecté cette
// latence aller-retour sur la boucle locale par netem : les clés prennent le
// suffixe « -<n>ms ».
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { join } from "node:path";
import { verser, absent } from "./commun.js";
import { SSH_PORT, CLE_CLIENTE } from "../wdio.conf.js";
import { cheminDistant } from "./sondes.js";
import { userInfo } from "node:os";

const executer = promisify(execFile);
const REPETITIONS = Number(process.env.BANC_SFTP_REPETITIONS ?? 3);

describe("Banc : SFTP", () => {
  it("S6 : montée et descente, gros fichier et petits fichiers", async function () {
    const bin = process.env.BANC_SFTP_BIN;
    if (!bin) {
      absent("S6", "BANC_SFTP_BIN absent (exemple banc_sftp non construit)");
      this.skip();
    }
    const suffixe = process.env.BANC_LATENCE_MS ? `-${process.env.BANC_LATENCE_MS}ms` : "";
    const dossier = cheminDistant(join(process.env.AVASH_E2E_SANDBOX));
    const { stdout } = await executer(bin,
      ["127.0.0.1", String(SSH_PORT), userInfo().username, CLE_CLIENTE, dossier, String(REPETITIONS)],
      { maxBuffer: 16 * 1024 * 1024, timeout: 30 * 60 * 1000 });
    for (const l of stdout.split("\n").filter((x) => x.startsWith("{"))) {
      const m = JSON.parse(l);
      verser(`${m.cle}${suffixe}`, { unite: m.unite, sens: m.sens }, [m.valeur]);
    }
  });
});
