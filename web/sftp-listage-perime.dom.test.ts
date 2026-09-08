// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : la branche de succès de sftpNavigate
// vérifie `sftpSession() !== s || s.sftpPath !== path` avant de toucher au DOM,
// mais le catch, lui, vidait #sftp-list et écrivait un statut d'erreur sans cette
// garde. Un listage périmé (autre dossier, autre onglet, ou permission refusée)
// qui rejette APRÈS qu'un listage plus récent a été rendu effaçait la liste
// courante et affichait une erreur qui ne la concernait pas ; il fallait cliquer
// Rafraîchir. Même défaut dans le catch de sftpOpenAt, qui renvoyait le panneau
// de l'onglet courant sur « . » sans vérifier que la session était toujours
// l'active. Ces tests lancent deux listages différés, résolvent le récent puis
// rejettent l'ancien, et exigent que le panneau garde le rendu récent.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";
import type { SftpEntry } from "./filters";

const invoke = vi.hoisted(() => vi.fn());
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
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a ni matchMedia, ni document.fonts, ni requestIdleCallback. */
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

const ENTREES_RECENT: SftpEntry[] = [
  { name: "recent-a.txt", is_dir: false, size: 10, modified: null },
  { name: "recent-b.txt", is_dir: false, size: 20, modified: null },
];

let sftpOpenAt: (s: Session, path: string) => Promise<void>;

/** Une session minimale : sftpNavigate ne lit que `id`, `sftpPath` et l'absence
 *  de `serie`. */
function sessionFactice(id: number): Session {
  return { id, sftpPath: "", serie: false } as unknown as Session;
}

/** Les entrées réelles, sans le « .. » de remontée (présent hors racine). */
function entrees(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>("#sftp-list .sftp-entry:not(.up)")];
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  const mod = await import("./sftp");
  sftpOpenAt = mod.sftpOpenAt;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockReset();
  state.sessions.clear();
});

describe("SFTP : un listage périmé qui échoue ne vide pas le panneau", () => {
  it("un_listage_perime_qui_echoue_ne_vide_pas_le_panneau", async () => {
    const s = sessionFactice(1);
    state.sessions.set(1, s);
    state.active = 1;

    // Deux listages sur la même session, résolus à la main : le lent (/lent)
    // rejettera, le récent (/recent) sera rendu avant.
    let rejeterLent!: (raison: unknown) => void;
    let resoudreRecent!: (v: SftpEntry[]) => void;
    invoke.mockImplementation((cmd: string, args: { path?: string }) => {
      if (cmd === "sftp_list" && args.path === "/lent") {
        return new Promise<SftpEntry[]>((_, rej) => (rejeterLent = rej));
      }
      if (cmd === "sftp_list" && args.path === "/recent") {
        return new Promise<SftpEntry[]>((res) => (resoudreRecent = res));
      }
      return Promise.resolve([]);
    });

    // sftpOpenAt avec un chemin explicite délègue directement à sftpNavigate.
    void sftpOpenAt(s, "/lent");
    void sftpOpenAt(s, "/recent");

    // Le récent est rendu.
    resoudreRecent(ENTREES_RECENT);
    await vi.waitFor(() => expect(entrees().length).toBe(2));

    // Puis l'ancien rejette, comme à la fermeture de son onglet (« Session 1
    // inconnue ») ou sur un simple « permission refusée ».
    rejeterLent("Session 1 inconnue");
    await vi.waitFor(() => Promise.resolve());
    await Promise.resolve();

    // La liste garde le rendu récent et le statut n'est pas passé en erreur.
    expect(entrees().length).toBe(2);
    expect(entrees().map((e) => e.querySelector(".nm")!.textContent)).toEqual(["recent-a.txt", "recent-b.txt"]);
    expect(document.getElementById("sftp-status")!.classList.contains("err")).toBe(false);
  });

  it("le_realpath_perime_qui_echoue_ne_ramene_pas_l_onglet_courant_a_la_racine", async () => {
    const s1 = sessionFactice(1);
    const s2 = sessionFactice(2);
    s2.sftpPath = "/home/moi";
    state.sessions.set(1, s1);
    state.sessions.set(2, s2);
    state.active = 1;

    let rejeterRealpath!: (raison: unknown) => void;
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "sftp_realpath") return new Promise<string>((_, rej) => (rejeterRealpath = rej));
      return Promise.resolve([]);
    });

    // sftpOpenAt sans chemin résout le home par sftp_realpath (lent).
    void sftpOpenAt(s1, "");
    // Bascule d'onglet : s2 devient l'active.
    state.active = 2;

    // Le realpath de l'ancien onglet échoue.
    rejeterRealpath("Session 1 inconnue");
    await vi.waitFor(() => Promise.resolve());
    await Promise.resolve();

    // Le panneau de l'onglet courant (s2) n'est pas renvoyé sur « . ».
    expect(s2.sftpPath).toBe("/home/moi");
  });
});
