// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-front-9, C-SIL-13) : l'écoute de la progression
// des transferts SFTP avalait son échec (`listen("sftp-progress").catch(() => {})`).
// Si la permission manque (capacités Tauri abîmées), toutes les lignes restaient
// à « … » jusqu'à la fin, sans un mot. Le PTY avait déjà son filet
// (`ptyListenError`) ; le panneau SFTP le dit désormais au premier transfert.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";
import type { SftpEntry } from "./filters";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((nom: string) =>
    nom === "sftp-progress"
      ? Promise.reject(new Error("event.listen not allowed"))
      : Promise.resolve(() => {}),
  ),
}));
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

const ENTREES: SftpEntry[] = [{ name: "gros.iso", is_dir: false, size: 1 << 30, modified: null }];

let sftpOpenAt: (s: Session, path: string) => Promise<void>;

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  sftpOpenAt = (await import("./sftp")).sftpOpenAt;
  (await import("./i18n")).setLangue("fr");
});

describe("progression SFTP : un échec d'écoute ne passe pas sous silence", () => {
  it("un_echec_d_ecoute_sftp_est_signale", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "sftp_list") return Promise.resolve(ENTREES);
      if (cmd === "sftp_download") return new Promise(() => {}); // transfert en cours
      return Promise.resolve([]);
    });
    const s = { id: 1, sftpPath: "", serie: false } as unknown as Session;
    state.sessions.set(1, s);
    state.active = 1;
    await sftpOpenAt(s, "/srv");
    const entree = await vi.waitFor(() => {
      const e = document.querySelector<HTMLElement>('#sftp-list .sftp-entry[aria-label="gros.iso"]');
      expect(e).toBeTruthy();
      return e!;
    });

    entree.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));

    const statut = document.getElementById("sftp-status")!;
    await vi.waitFor(() => expect(statut.textContent).toContain("event.listen not allowed"));
    expect(statut.className).toContain("err");
  });
});
