// @vitest-environment jsdom
// Audit du 27 novembre 2026 (WI-3), choix du mainteneur : sous Windows, le
// greffon lance l'installeur NSIS puis termine Avash d'office. Un clic sur
// « Installer » coupait toutes les sessions, transferts compris, sans le dire.
// Si des sessions sont ouvertes, Avash demande désormais : installer
// maintenant (onglets fermés d'abord), ou au prochain lancement.
import { describe, it, expect, beforeEach, vi } from "vitest";

const check = vi.hoisted(() => vi.fn());
const invoke = vi.hoisted(() => vi.fn());
const askConfirm = vi.hoisted(() => vi.fn());
const notify = vi.hoisted(() => vi.fn());
const notifyErreur = vi.hoisted(() => vi.fn());
const estWindows = vi.hoisted(() => vi.fn(() => true));

vi.mock("@tauri-apps/plugin-updater", () => ({ check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./dialogues", () => ({ askConfirm }));
vi.mock("./notifications", () => ({ notify, notifyErreur }));
vi.mock("./vnc-clavier", () => ({ estWindows }));
vi.mock("./i18n", () => ({ t: (cle: string, v?: Record<string, unknown>) => (v ? `${cle} ${JSON.stringify(v)}` : cle) }));

type Maj = typeof import("./maj");

function miseAJour() {
  return {
    version: "9.9.9",
    currentVersion: "1.2.3",
    body: "",
    download: vi.fn().mockResolvedValue(undefined),
    install: vi.fn().mockResolvedValue(undefined),
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
  };
}

async function charger(): Promise<Maj> {
  vi.resetModules();
  return import("./maj");
}

describe("mise à jour sous Windows avec des sessions ouvertes", () => {
  beforeEach(() => {
    for (const f of [check, invoke, askConfirm, notify, notifyErreur]) f.mockReset();
    estWindows.mockReturnValue(true);
    invoke.mockResolvedValue("greffon");
    localStorage.clear();
    document.body.innerHTML = `<span id="app-version">1.2.3</span>`;
  });

  it("demande avant de fermer les sessions, et les ferme avant d'installer maintenant", async () => {
    const maj = await charger();
    const ordre: string[] = [];
    const u = miseAJour();
    u.install.mockImplementation(() => { ordre.push("installer"); return Promise.resolve(); });
    maj.brancherSessionsPourMaj({ nombre: () => 3, fermerTout: () => { ordre.push("fermer"); return Promise.resolve(); } });
    check.mockResolvedValue(u);
    askConfirm.mockResolvedValueOnce(true).mockResolvedValueOnce(true); // installer ? puis maintenant

    document.getElementById("app-version")!.dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(u.install).toHaveBeenCalled());

    expect(askConfirm).toHaveBeenCalledTimes(2);
    const [question, options] = askConfirm.mock.calls[1];
    expect(question).toContain("maj-sessions-fermees-pluriel");
    expect(question).toContain('"n":3');
    expect(options).toMatchObject({ ok: "maj-installer-maintenant", annuler: "maj-au-prochain-lancement" });
    expect(ordre).toEqual(["fermer", "installer"]);
    expect(u.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("au prochain lancement : rien n'est fermé ni installé, et la version est retenue", async () => {
    const maj = await charger();
    const fermerTout = vi.fn().mockResolvedValue(undefined);
    const u = miseAJour();
    maj.brancherSessionsPourMaj({ nombre: () => 1, fermerTout });
    check.mockResolvedValue(u);
    askConfirm.mockResolvedValueOnce(true).mockResolvedValueOnce(false);

    document.getElementById("app-version")!.dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(notify).toHaveBeenCalled());

    expect(askConfirm.mock.calls[1][0]).toContain("maj-sessions-fermees ");
    expect(fermerTout).not.toHaveBeenCalled();
    expect(u.install).not.toHaveBeenCalled();
    expect(localStorage.getItem("avash.maj.au-prochain-lancement")).toBe("9.9.9");
  });

  it("au lancement suivant, la mise à jour reportée s'installe sans question si aucune session n'est ouverte", async () => {
    localStorage.setItem("avash.maj.au-prochain-lancement", "9.9.9");
    const u = miseAJour();
    check.mockResolvedValue(u);
    await charger(); // le chargement de la page lance l'installation reportée
    await vi.waitFor(() => expect(u.install).toHaveBeenCalled());
    expect(askConfirm).not.toHaveBeenCalled();
    expect(localStorage.getItem("avash.maj.au-prochain-lancement")).toBeNull();
  });

  it("sans session ouverte, l'installation part sans seconde question", async () => {
    const maj = await charger();
    const u = miseAJour();
    maj.brancherSessionsPourMaj({ nombre: () => 0, fermerTout: async () => {} });
    check.mockResolvedValue(u);
    askConfirm.mockResolvedValueOnce(true);

    document.getElementById("app-version")!.dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(u.install).toHaveBeenCalled());
    expect(askConfirm).toHaveBeenCalledTimes(1);
  });

  it("un téléchargement en échec ne ferme aucune session", async () => {
    const maj = await charger();
    const fermerTout = vi.fn();
    const u = miseAJour();
    u.download.mockRejectedValue(new Error("réseau coupé"));
    maj.brancherSessionsPourMaj({ nombre: () => 2, fermerTout });
    check.mockResolvedValue(u);
    askConfirm.mockResolvedValueOnce(true);

    document.getElementById("app-version")!.dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(notifyErreur).toHaveBeenCalled());
    expect(fermerTout).not.toHaveBeenCalled();
    expect(u.install).not.toHaveBeenCalled();
  });
});
