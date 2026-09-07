// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : le panneau SFTP était inaccessible au
// clavier. Les entrées `.sftp-entry` étaient des `div` clonées sans tabindex, ni
// rôle, ni gestion des touches (seuls click/dblclick/contextmenu étaient
// délégués) : un utilisateur clavier atteignait le champ chemin et les boutons du
// bas, mais ne pouvait ni entrer dans un dossier, ni télécharger, ni ouvrir le
// menu contextuel d'une entrée. La barre latérale avait pourtant déjà ce
// traitement (main.ts, rendreAtteignableAuClavier). Ces tests verrouillent que
// chaque entrée est un bouton focalisable, que les flèches déplacent le focus,
// qu'Entrée équivaut au double-clic (naviguer ou télécharger) et que Maj+F10
// ouvre le menu au clavier.
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

// Deux entrées : un dossier « etc » et un fichier « hosts.txt ». sortSftpEntries
// place les dossiers d'abord, donc l'ordre DOM est : « .. », etc, hosts.txt.
const ENTREES: SftpEntry[] = [
  { name: "hosts.txt", is_dir: false, size: 42, modified: null },
  { name: "etc", is_dir: true, size: 0, modified: null },
];

let sftpOpenAt: (s: Session, path: string) => Promise<void>;

/** Une session minimale : sftpNavigate ne lit que `id`, `sftpPath` et l'absence
 *  de `serie`. Le reste de la Session (terminal, onglet…) n'est pas touché. */
function sessionFactice(): Session {
  return { id: 1, sftpPath: "", serie: false } as unknown as Session;
}

function entrees(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>("#sftp-list .sftp-entry")];
}

function touche(el: HTMLElement, key: string, extra: Partial<KeyboardEventInit> = {}): void {
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...extra }));
}

/** Monte la liste sur le chemin donné et attend son rendu. */
async function ouvrir(path: string): Promise<void> {
  const s = sessionFactice();
  state.sessions.set(1, s);
  state.active = 1;
  await sftpOpenAt(s, path);
  await vi.waitFor(() => expect(entrees().length).toBeGreaterThan(0));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // ./main fixe l'ordre d'évaluation du cycle des modules (cf. les autres tests
  // DOM) ; il importe ./sftp, dont on récupère ensuite l'export.
  await import("./main");
  const mod = await import("./sftp");
  sftpOpenAt = mod.sftpOpenAt;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "sftp_list") return Promise.resolve(ENTREES);
    if (cmd === "sftp_download") return Promise.resolve("téléchargé");
    return Promise.resolve([]);
  });
  document.getElementById("sftp-context")!.classList.remove("open");
});

describe("panneau SFTP : entrées atteignables au clavier", () => {
  it("fait de chaque entrée un bouton focalisable et annoncé par son nom", async () => {
    await ouvrir("/root");
    const els = entrees();
    // « .. » plus les deux entrées.
    expect(els.length).toBe(3);
    for (const el of els) {
      expect(el.getAttribute("role")).toBe("button");
      expect(el.getAttribute("aria-label")).toBeTruthy();
    }
    // Un seul arrêt de tabulation (tabindex glissant) : la première à 0.
    expect(els[0].tabIndex).toBe(0);
    expect(els.slice(1).every((el) => el.tabIndex === -1)).toBe(true);
  });

  it("déplace le focus à la flèche bas et surligne l'entrée focalisée (.hl)", async () => {
    await ouvrir("/root");
    const els = entrees();
    els[0].focus();
    touche(els[0], "ArrowDown");
    expect(document.activeElement).toBe(els[1]);
    expect(els[1].classList.contains("hl")).toBe(true);
    // Le surlignage suit le focus, ne s'accumule pas.
    expect(els[0].classList.contains("hl")).toBe(false);
    expect(els[1].tabIndex).toBe(0);
  });

  it("va aux extrémités avec Fin et Origine", async () => {
    await ouvrir("/root");
    const els = entrees();
    els[0].focus();
    touche(els[0], "End");
    expect(document.activeElement).toBe(els[2]);
    touche(els[2], "Home");
    expect(document.activeElement).toBe(els[0]);
  });

  it("descend dans un dossier avec Entrée (équivalent du double-clic)", async () => {
    await ouvrir("/root");
    const dossier = entrees()[1]; // « etc » (dossier, en tête après « .. »)
    invoke.mockClear();
    touche(dossier, "Enter");
    await vi.waitFor(() =>
      expect(invoke.mock.calls.some((c) => c[0] === "sftp_list" && (c[1] as { path: string }).path === "/root/etc")).toBe(true),
    );
  });

  it("télécharge un fichier avec Entrée", async () => {
    await ouvrir("/root");
    const fichier = entrees()[2]; // « hosts.txt »
    invoke.mockClear();
    touche(fichier, "Enter");
    await vi.waitFor(() => expect(invoke.mock.calls.some((c) => c[0] === "sftp_download")).toBe(true));
  });

  it("ouvre le menu contextuel au clavier avec Maj+F10 et y place le focus", async () => {
    await ouvrir("/root");
    const fichier = entrees()[2];
    fichier.focus();
    touche(fichier, "F10", { shiftKey: true });
    const menu = document.getElementById("sftp-context")!;
    expect(menu.classList.contains("open")).toBe(true);
    // Le premier item visible du menu prend le focus.
    const premierItem = menu.querySelector<HTMLElement>("[data-act]:not([hidden])");
    expect(document.activeElement).toBe(premierItem);
  });
});
