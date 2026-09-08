// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : deux gestes de copie vers le
// presse-papiers avalaient un rejet de navigator.clipboard.writeText (sous
// WebKitGTK l'API peut refuser : permission, contexte). « Copier la clé
// publique » (cles.ts) faisait un `await` sans catch — rejet non géré, le
// libellé ne passait jamais à « copiée ✓ » et l'utilisateur collait l'ancien
// contenu dans authorized_keys sans le savoir. « Copier le chemin » du menu
// SFTP (sftp.ts) avait un second callback vide `() => {}` : aucun statut.
// Ces tests verrouillent qu'un refus d'écriture se dit à l'utilisateur.
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";

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

/** Pose un presse-papiers dont l'écriture aboutit ou échoue à volonté. */
function poserPressePapiers(reussit: boolean): ReturnType<typeof vi.fn> {
  const writeText = vi.fn(() => (reussit ? Promise.resolve() : Promise.reject(new Error("refus"))));
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText, readText: vi.fn(() => Promise.resolve("")) },
  });
  return writeText;
}

const CLE = {
  name: "id_ed25519",
  path: "/home/x/.ssh/id_ed25519",
  public_line: "ssh-ed25519 AAAAC3Nz x",
  mode: "600",
};

let keysOpen: () => Promise<void>;
let t: (cle: string, vars?: Record<string, string | number>) => string;
let sftpMod: typeof import("./sftp");

/** Une session minimale : sftpSession ne lit que l'absence de `serie`. */
function sessionFactice(): Session {
  return { id: 1, sftpPath: "", serie: false } as unknown as Session;
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // ./main fixe l'ordre d'évaluation du cycle des modules (cf. les autres
  // tests DOM) ; il importe ./sftp, dont on récupère ensuite l'export.
  await import("./main");
  sftpMod = await import("./sftp");
  const cles = await import("./cles");
  keysOpen = cles.keysOpen;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
  t = i18n.t;
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "keys_list") return Promise.resolve([CLE]);
    return Promise.resolve([]);
  });
});

afterEach(() => {
  vi.useRealTimers();
});

function boutonCopieCle(): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>("#key-list .kcopy")!;
}

describe("copie de la clé publique", () => {
  it("refus du presse-papiers : affiche l'erreur et laisse le libellé inchangé", async () => {
    poserPressePapiers(false);
    await keysOpen();
    const btn = boutonCopieCle();
    const libelle = btn.textContent;
    btn.click();
    await vi.waitFor(() => expect(document.getElementById("k-error")!.hidden).toBe(false));
    expect(document.getElementById("k-error")!.textContent).toBe(t("cles-copie-impossible"));
    // Le bouton n'a pas menti : il n'a jamais affiché « copiée ✓ ».
    expect(btn.textContent).toBe(libelle);
  });

  it("succès : passe à « copiée ✓ » puis revient après 1500 ms", async () => {
    vi.useFakeTimers();
    poserPressePapiers(true);
    await keysOpen();
    const btn = boutonCopieCle();
    btn.click();
    // Laisse tourner le callback de résolution de writeText.
    await Promise.resolve();
    await Promise.resolve();
    expect(btn.textContent).toBe(t("cles-copiee"));
    expect(document.getElementById("k-error")!.hidden).toBe(true);
    vi.advanceTimersByTime(1500);
    expect(btn.textContent).toBe(t("cles-copier-publique"));
  });
});

describe("copie du chemin dans le menu SFTP", () => {
  it("refus du presse-papiers : signale l'échec en statut err", async () => {
    poserPressePapiers(false);
    state.sessions.set(1, sessionFactice());
    state.active = 1;
    sftpMod.sftp.ctx = { entry: null, path: "/etc" };
    const item = document.querySelector<HTMLElement>('#sftp-context [data-act="copy"]')!;
    item.click();
    const statut = document.getElementById("sftp-status")!;
    await vi.waitFor(() => expect(statut.className).toContain("err"));
    expect(statut.textContent).toBe(t("enregistrements-copie-impossible"));
  });
});
