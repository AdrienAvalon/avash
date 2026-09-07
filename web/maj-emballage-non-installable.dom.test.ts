// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026, complété par la relecture : la
// vérification des mises à jour appelait checkUpdate() sans condition et
// proposait l'installation intégrée même sur les emballages où le greffon
// updater ne peut pas aboutir. Le manifeste latest.json ne sert que l'AppImage
// (Linux), le setup NSIS (Windows) et l'app (macOS) : les autres canaux
// téléchargeaient une AppImage que l'installeur refuse (fausse annonce
// « Version X disponible » puis échec après ~100 Mo). Deux cas distincts :
//  - Flatpak, AUR, Flathub, .deb/.rpm : c'est le gestionnaire de paquets qui met
//    à jour (canal « gestionnaire ») ;
//  - archive portable Windows (avash-ui.exe brut, non estampillé) : le greffon
//    installerait le setup NSIS ailleurs (%LOCALAPPDATA%\Programs) en laissant le
//    dossier portable en arrière, qui reproposerait la mise à jour à chaque
//    lancement (canal « archive ») : on renvoie vers la page de release.
// Le front interroge désormais la commande canal_de_mise_a_jour et, hors du
// canal « greffon », court-circuite la vérification. Ces tests verrouillent que
// checkUpdate() et downloadAndInstall ne sont alors jamais appelés, que chaque
// canal affiche son message, et que le chemin greffon (AppImage/Windows installé)
// reste intact.
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

describe("la vérification des mises à jour respecte l'emballage", () => {
  beforeEach(() => {
    check.mockReset();
    relaunch.mockReset();
    invoke.mockReset();
    askConfirm.mockReset();
    notify.mockReset();
    notifyErreur.mockReset();
    monterDom();
  });

  it("canal « gestionnaire » (Flatpak/AUR/Flathub/.deb/.rpm) : ni checkUpdate ni téléchargement, on renvoie au gestionnaire de paquets", async () => {
    invoke.mockResolvedValue("gestionnaire"); // canal_de_mise_a_jour
    await chargerEtCliquer();
    await vi.waitFor(() => expect(notify).toHaveBeenCalled());

    expect(invoke).toHaveBeenCalledWith("canal_de_mise_a_jour");
    expect(check).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith("maj-gere-par-paquet", "info");
    expect(notifyErreur).not.toHaveBeenCalled();
  });

  it("canal « archive » (portable Windows) : ni checkUpdate ni téléchargement, on renvoie à la page de release", async () => {
    invoke.mockResolvedValue("archive"); // canal_de_mise_a_jour
    await chargerEtCliquer();
    await vi.waitFor(() => expect(notify).toHaveBeenCalled());

    expect(invoke).toHaveBeenCalledWith("canal_de_mise_a_jour");
    expect(check).not.toHaveBeenCalled();
    // Message distinct : surtout pas « gestionnaire de paquets » sur Windows.
    expect(notify).toHaveBeenCalledWith("maj-archive-portable", "info");
    expect(notify).not.toHaveBeenCalledWith("maj-gere-par-paquet", "info");
    expect(notifyErreur).not.toHaveBeenCalled();
  });

  it("canal « greffon » (AppImage/Windows installé) : checkUpdate est appelée et l'installation intégrée reste possible", async () => {
    invoke.mockResolvedValue("greffon"); // canal_de_mise_a_jour
    const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
    check.mockResolvedValue({
      version: "9.9.9",
      currentVersion: "1.2.3",
      body: "notes",
      downloadAndInstall,
    });
    askConfirm.mockResolvedValue(true); // installer, puis redémarrer
    relaunch.mockResolvedValue(undefined);

    await chargerEtCliquer();
    await vi.waitFor(() => expect(downloadAndInstall).toHaveBeenCalled());

    expect(check).toHaveBeenCalledTimes(1);
    expect(relaunch).toHaveBeenCalled();
  });
});
