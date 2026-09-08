// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026, complété par la relecture : un seul
// try englobait checkUpdate(), downloadAndInstall() et relaunch(), et le catch
// annonçait toujours « Vérification des mises à jour impossible ». Une signature
// invalide, un réseau coupé pendant le téléchargement ou un AppImage non
// inscriptible étaient donc qualifiés d'échec de vérification, alors que la
// vérification avait réussi et qu'une mise à jour venait d'être annoncée :
// l'utilisateur recliquait, la vérification réussissait à nouveau, et la boucle
// recommençait sans qu'il comprenne que c'était l'installation qui échouait.
// Ces tests verrouillent que chaque étape a désormais son propre message d'échec.
import { describe, it, expect, beforeEach, vi } from "vitest";

const check = vi.hoisted(() => vi.fn());
const relaunch = vi.hoisted(() => vi.fn());
const invoke = vi.hoisted(() => vi.fn());
const askConfirm = vi.hoisted(() => vi.fn());
const notify = vi.hoisted(() => vi.fn());
const notifyErreur = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/plugin-updater", () => ({ check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./dialogues", () => ({ askConfirm }));
vi.mock("./notifications", () => ({ notify, notifyErreur }));
// t renvoie sa clé : on vérifie le contrôle de flux, pas la traduction (déjà
// couverte par i18n.test.ts), et on évite les écouteurs câblés par i18n.
vi.mock("./i18n", () => ({ t: (cle: string) => cle }));

/** Le seul élément lu par maj.ts : la pastille de version qui porte le clic. */
function monterDom() {
  document.body.innerHTML = `<span id="app-version">1.2.3</span>`;
}

/** (Re)charge maj.ts sur le DOM courant puis déclenche le clic de vérification. */
async function chargerEtCliquer() {
  vi.resetModules();
  await import("./maj");
  document.getElementById("app-version")!.dispatchEvent(new Event("click"));
}

describe("un échec d'installation ou de redémarrage n'est pas annoncé comme un échec de vérification", () => {
  beforeEach(() => {
    check.mockReset();
    relaunch.mockReset();
    invoke.mockReset();
    askConfirm.mockReset();
    notify.mockReset();
    notifyErreur.mockReset();
    monterDom();
    invoke.mockResolvedValue("greffon"); // canal_de_mise_a_jour : chemin installable
  });

  it("le téléchargement/installation qui échoue affiche le message d'installation, pas celui de vérification", async () => {
    const downloadAndInstall = vi.fn().mockRejectedValue(new Error("réseau coupé"));
    check.mockResolvedValue({ version: "9.9.9", currentVersion: "1.2.3", body: "notes", downloadAndInstall });
    askConfirm.mockResolvedValue(true); // « Installer » confirmé

    await chargerEtCliquer();
    await vi.waitFor(() => expect(notifyErreur).toHaveBeenCalled());

    expect(downloadAndInstall).toHaveBeenCalled();
    expect(notifyErreur).toHaveBeenCalledWith("maj-installation-impossible");
    expect(notifyErreur).not.toHaveBeenCalledWith("maj-verification-impossible");
    expect(relaunch).not.toHaveBeenCalled();
  });

  it("le redémarrage qui échoue affiche le message de redémarrage (mise à jour déjà installée), pas celui de vérification", async () => {
    const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
    check.mockResolvedValue({ version: "9.9.9", currentVersion: "1.2.3", body: "notes", downloadAndInstall });
    askConfirm.mockResolvedValue(true); // « Installer » puis « Redémarrer » confirmés
    relaunch.mockRejectedValue(new Error("processus refusé"));

    await chargerEtCliquer();
    await vi.waitFor(() => expect(notifyErreur).toHaveBeenCalled());

    expect(downloadAndInstall).toHaveBeenCalled();
    expect(notifyErreur).toHaveBeenCalledWith("maj-redemarrage-impossible");
    expect(notifyErreur).not.toHaveBeenCalledWith("maj-verification-impossible");
  });

  it("la vérification qui échoue garde le message de vérification (non-régression)", async () => {
    check.mockRejectedValue(new Error("hors ligne"));

    await chargerEtCliquer();
    await vi.waitFor(() => expect(notifyErreur).toHaveBeenCalled());

    expect(notifyErreur).toHaveBeenCalledWith("maj-verification-impossible");
    expect(notifyErreur).not.toHaveBeenCalledWith("maj-installation-impossible");
  });
});
