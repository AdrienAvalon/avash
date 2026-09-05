// Les vues du README : accueil, terminal SSH, bureau RDP, et les cadres de la
// démonstration animée, pris aux moments clés le long du même parcours. Voir
// wdio.captures.conf.js et scripts/captures-readme.sh.
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { attendreBureauConnecte, doubleCliquer, doubleCliquerHote } from "../specs/helpers.js";

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
    await doubleCliquerHote("test-ssh");
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

  it("panneau SFTP", async () => {
    // Un arbre de fichiers plausible, servi par le sshd local : le panneau
    // montre une arborescence de déploiement, pas le disque du mainteneur.
    const racine = mkdtempSync(join(tmpdir(), "avash-captures-"));
    const site = join(racine, "srv", "web-1");
    for (const d of ["releases/2026-09-04", "releases/2026-08-28", "backups", "config"]) {
      mkdirSync(join(site, d), { recursive: true });
    }
    writeFileSync(join(site, "config", "nginx.conf"), "server {\n  listen 443 ssl;\n}\n");
    writeFileSync(join(site, "config", "docker-compose.yml"), "services:\n  web:\n    image: nginx\n");
    writeFileSync(join(site, "backups", "db-2026-09-04.sql.gz"), Buffer.alloc(3 * 1024 * 1024 + 517, 7));
    writeFileSync(join(site, "backups", "db-2026-08-28.sql.gz"), Buffer.alloc(2 * 1024 * 1024 + 88, 9));
    writeFileSync(join(site, "releases", "2026-09-04", "app.tar.gz"), Buffer.alloc(11 * 1024 * 1024 + 41, 3));
    writeFileSync(join(site, "deploy.sh"), "#!/bin/sh\nset -e\n");
    writeFileSync(join(site, "README.md"), "# web-1\n");

    await $("#sftp-toggle").click();
    await browser.waitUntil(async () => (await $("#sftp-panel").getAttribute("class")).includes("open"),
      { timeout: 5000, timeoutMsg: "panneau SFTP jamais ouvert" });
    // La première liste (le dossier personnel) remplit aussi la barre de
    // chemin : taper avant qu'elle n'arrive, c'est se faire écraser.
    await browser.waitUntil(async () => (await $$("#sftp-list .sftp-entry")).length > 0,
      { timeout: 15000, timeoutMsg: "première liste SFTP jamais arrivée" });
    const barre = $("#sftp-path");
    // Valeur posée puis Entrée dispatchée : tapé touche par touche, un long
    // chemin perd des lettres sur une machine chargée.
    await browser.execute((c) => {
      const el = document.getElementById("sftp-path");
      el.focus();
      el.value = c;
      el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    }, site);
    await browser.waitUntil(async () => (await $$("#sftp-list .sftp-entry")).length >= 5,
      { timeout: 10000, timeoutMsg: "le panneau n'a pas listé l'arbre" });
    // Le chemin affiché est celui du bac à sable temporaire : on lui donne
    // l'allure d'un serveur, le temps de la capture.
    await browser.execute((chemin) => { document.querySelector("#sftp-path").value = chemin; }, "/srv/web-1");
    await pause(500);
    await cadre(2);
    // Une sauvegarde téléchargée : la file des transferts apparaît sous la
    // liste, avec sa ligne, sa progression et sa vitesse.
    const lignes = await $$("#sftp-list .sftp-entry");
    for (const el of lignes) {
      if ((await el.$(".nm").getProperty("textContent")) !== "backups") continue;
      await doubleCliquer(el);
      break;
    }
    await browser.waitUntil(async () => (await $$("#sftp-list .sftp-entry")).length >= 2 && (await barre.getValue()).endsWith("backups"),
      { timeout: 10000, timeoutMsg: "le panneau n'est pas entré dans backups" });
    await browser.execute((chemin) => { document.querySelector("#sftp-path").value = chemin; }, "/srv/web-1/backups");
    for (const el of await $$("#sftp-list .sftp-entry")) {
      if ((await el.$(".nm").getProperty("textContent")) !== "db-2026-09-04.sql.gz") continue;
      await browser.execute((e) => {
        e.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 200, clientY: 200 }));
      }, el);
      await $("#sftp-context").waitForDisplayed({ timeout: 3000 });
      await $('#sftp-context [data-act="download"]').click();
      break;
    }
    await browser.waitUntil(async () => (await $$("#sftp-transferts .sftp-transfert")).length > 0,
      { timeout: 10000, timeoutMsg: "aucune ligne de transfert" });
    await pause(700);
    await browser.saveScreenshot(`${DOSSIER}/sftp.png`);
    await cadre(3);
    await $("#sftp-toggle").click();
    await pause(300);
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
