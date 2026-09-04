// Les vues du README : accueil, terminal SSH, bureau RDP, et les cadres de la
// démonstration animée, pris aux moments clés le long du même parcours. Voir
// wdio.captures.conf.js et scripts/captures-readme.sh.
import { writeFileSync } from "node:fs";
import { findHostRow, attendreBureauConnecte } from "../specs/helpers.js";

const DOSSIER = process.env.CAPTURES_DOSSIER;
const CADRES = process.env.CAPTURES_CADRES;
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

let numero = 0;
// Un cadre de la démonstration ; `fois` copies pour tenir la vue à l'écran
// (le montage joue les cadres à cadence fixe).
async function cadre(fois = 1) {
  if (!CADRES) return;
  const png = await browser.saveScreenshot(`${CADRES}/${String(++numero).padStart(3, "0")}.png`);
  for (let i = 1; i < fois; i++) {
    writeFileSync(`${CADRES}/${String(++numero).padStart(3, "0")}.png`, png);
  }
}

async function theme(voulu) {
  for (let i = 0; i < 3; i++) {
    const actuel = await browser.execute(() => document.documentElement.getAttribute("data-theme"));
    if (actuel === voulu) return;
    await $("#theme-toggle").click();
  }
}

describe("captures du README", () => {
  before(async () => {
    if (!DOSSIER) throw new Error("CAPTURES_DOSSIER absent : passer par scripts/captures-readme.sh");
    try { await browser.setWindowSize(1280, 800); } catch { /* pilote sans redimensionnement */ }
    await theme("dark");
    await pause(600);
  });

  it("accueil", async () => {
    await browser.saveScreenshot(`${DOSSIER}/accueil.png`);
    await cadre(4);
  });

  it("terminal SSH", async () => {
    await (await findHostRow("test-ssh")).doubleClick();
    await pause(300);
    await cadre();
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });
    await pause(1000);
    await cadre(2);
    // Une invite et des commandes neutres : la machine derrière le sshd local
    // est le poste du mainteneur, on ne montre ni son nom ni ses disques. Le
    // shell de connexion peut être fish : on passe par bash pour l'invite.
    // Frappe touche par touche : envoyées d'un bloc, des lettres se perdent.
    const taper = async (texte) => {
      for (const c of texte) {
        await browser.keys(c);
        await pause(12);
      }
      await browser.keys("Enter");
    };
    for (const commande of [
      "exec bash --norc",
      "export PS1='deploy@web-1:~$ '; clear",
      "uptime",
      "grep -E '^(NAME|VERSION_ID)=' /etc/os-release",
      "ls -la /etc/ssh",
    ]) {
      await taper(commande);
      await pause(600);
      await cadre();
    }
    await pause(800);
    await browser.saveScreenshot(`${DOSSIER}/terminal-ssh.png`);
    await cadre(3);
  });

  it("bureau RDP", async function () {
    const hote = process.env.CAPTURES_RDP_HOTE;
    if (!hote) return this.skip();
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await pause(400);
    await cadre(2);
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await pause(300);
    // Le formulaire RDP vide : pas de cadre une fois rempli, l'adresse du
    // parc et le compte n'ont rien à faire dans la démonstration.
    await cadre(2);
    await $("#m-addr").setValue(hote);
    await $("#m-port").setValue("3389");
    await $("#m-user").setValue(process.env.CAPTURES_RDP_UTILISATEUR);
    await $("#m-password").setValue(process.env.CAPTURES_RDP_MDP);
    await $("#m-submit").click();
    await pause(800);
    await cadre();
    const canvas = await $(".rdp-container canvas");
    await canvas.waitForExist({ timeout: 30000, timeoutMsg: "aucun canvas RDP" });
    await attendreBureauConnecte();
    await pause(6000);
    // Un clic dans le bureau : Windows garde pour le premier geste ce qu'il a
    // à montrer (l'avertissement de connexion et son bouton OK). L'écran
    // d'avertissement est centré et de taille fixe : OK est 75 px sous le
    // centre, quelle que soit la taille du bureau.
    await canvas.click();
    await pause(3000);
    await cadre(2);
    await canvas.click({ x: 0, y: 75 });
    await pause(12000);
    await browser.saveScreenshot(`${DOSSIER}/bureau-rdp.png`);
    await cadre(6);
  });
});
