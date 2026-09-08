// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : la fiche d'édition d'un bureau RDP
// faisait `rdp_password_save(...).catch(() => {})`, `rdp_password_forget(...)`
// et `rdp_password_move(...).catch(() => {})`. Un trousseau injoignable ou qui
// refuse l'écriture laissait la fiche se fermer « bureau enregistré » alors que
// le mot de passe n'était pas mémorisé (ou restait sous l'ancien compte après un
// changement de port/hôte/utilisateur) : la connexion suivante le redemandait
// sans explication. Ces tests exigent qu'un rejet du trousseau émette une
// notification d'erreur, tout en gardant le bureau sauvé (`loadHosts` appelé,
// donc `rdp_hosts` relu).
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

const invoke = vi.hoisted(() => vi.fn());
const notifyErreur = vi.hoisted(() => vi.fn());
const notify = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: () => Promise.resolve(false),
    minimize: vi.fn(),
    toggleMaximize: () => Promise.resolve(),
    close: vi.fn(),
    onResized: () => Promise.resolve(() => {}),
    onFocusChanged: () => Promise.resolve(() => {}),
    startResizeDragging: vi.fn(),
    setFullscreen: () => Promise.resolve(),
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));
vi.mock("./notifications", () => ({ notifyErreur, notify }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

function shimsNavigateur(): void {
  window.matchMedia = ((): MediaQueryList =>
    ({
      matches: false,
      media: "",
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList) as typeof window.matchMedia;
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: () => Promise.resolve([]), ready: Promise.resolve(), add: () => {} },
  });
  window.requestIdleCallback = ((cb: () => void) => setTimeout(cb, 0)) as typeof window.requestIdleCallback;
}

function form(): HTMLFormElement {
  return document.getElementById("rdp-edit-form") as HTMLFormElement;
}
function champ(id: string): HTMLInputElement {
  return document.getElementById(id) as HTMLInputElement;
}

/** Renseigne la fiche puis soumet. `ancien` porte le compte d'origine
 *  (dataset) : l'égalité avec le compte courant décide de la migration. */
async function soumettre(opts: {
  host: string; port: string; user: string; motDePasse: string;
  ancien: { host: string; port: string; user: string; proto: string };
}): Promise<void> {
  champ("re-id").value = "bureau-1";
  champ("re-name").value = "Bureau";
  champ("re-addr").value = opts.host;
  champ("re-user").value = opts.user;
  champ("re-port").value = opts.port;
  champ("re-password").value = opts.motDePasse;
  (document.getElementById("re-proto") as HTMLSelectElement).value = "rdp";
  const f = form();
  f.dataset.oldHost = opts.ancien.host;
  f.dataset.oldPort = opts.ancien.port;
  f.dataset.oldUser = opts.ancien.user;
  f.dataset.oldProto = opts.ancien.proto;
  f.dispatchEvent(new Event("submit", { cancelable: true }));
  // Laisser filer la chaîne de promesses du gestionnaire (invoke moqués).
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("rdp_hosts"));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockReset();
  notifyErreur.mockReset();
  notify.mockReset();
  // Tout réussit par défaut, sauf le rejet posé par chaque test.
  invoke.mockResolvedValue([]);
});

describe("Fiche RDP : l'échec du trousseau ne doit plus être avalé", () => {
  it("notifie quand rdp_password_save échoue, et enregistre quand même le bureau", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "rdp_password_save") return Promise.reject(new Error("Écriture dans le trousseau impossible"));
      return Promise.resolve([]);
    });

    // Compte inchangé : seule la sauvegarde du mot de passe est tentée.
    await soumettre({
      host: "10.0.0.5", port: "3389", user: "adrien", motDePasse: "secret",
      ancien: { host: "10.0.0.5", port: "3389", user: "adrien", proto: "rdp" },
    });

    // L'échec est signalé (message de mémorisation), plus avalé.
    expect(notifyErreur).toHaveBeenCalledTimes(1);
    expect(notifyErreur.mock.calls[0][0]).toContain("Mémorisation impossible");
    // Le bureau reste enregistré : rdp_host_save émis et la liste rechargée.
    expect(invoke).toHaveBeenCalledWith("rdp_host_save", expect.anything());
    expect(invoke).toHaveBeenCalledWith("rdp_hosts");
  });

  it("notifie quand rdp_password_move échoue, en disant que le mot de passe reste sous l'ancien compte", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "rdp_password_move") return Promise.reject(new Error("Écriture dans le trousseau impossible"));
      return Promise.resolve([]);
    });

    // Changement de port sans nouveau mot de passe : migration du secret.
    await soumettre({
      host: "10.0.0.5", port: "3390", user: "adrien", motDePasse: "",
      ancien: { host: "10.0.0.5", port: "3389", user: "adrien", proto: "rdp" },
    });

    expect(invoke).toHaveBeenCalledWith("rdp_password_move", expect.anything());
    expect(notifyErreur).toHaveBeenCalledTimes(1);
    expect(notifyErreur.mock.calls[0][0]).toContain("ancien compte");
    expect(invoke).toHaveBeenCalledWith("rdp_hosts");
  });
});
