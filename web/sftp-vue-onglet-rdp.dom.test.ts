// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : en passant d'un onglet SSH (panneau
// SFTP ouvert) à un onglet RDP, `focusRdp` ne resynchronisait ni le bouton
// « Fichiers (SFTP) » ni le panneau. Le bouton restait cliquable et « actif »
// alors qu'un clic ne faisait rien (aucun SFTP pour un bureau distant), et le
// panneau restait ouvert à côté du bureau, affichant les fichiers de la session
// SSH précédente. `closeRdp` avait le même trou quand il ne restait plus aucun
// onglet. Ces tests verrouillent `sftpAppliquerVue` : bouton grisé et panneau
// masqué quand l'onglet courant n'a pas de système de fichiers, tout en gardant
// `sftp.open` comme préférence restaurée au retour sur l'onglet SSH.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
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

let sftp: { open: boolean };
let sftpAppliquerVue: () => void;

/** Une session SSH minimale : `sftpSession()` ne lit que la présence dans
 *  `state.sessions` et l'absence de `serie`. */
function sessionSsh(id: number): Session {
  return { id, sftpPath: "", serie: false } as unknown as Session;
}

function toggle(): HTMLButtonElement {
  return document.getElementById("sftp-toggle") as HTMLButtonElement;
}
function panneau(): HTMLElement {
  return document.getElementById("sftp-panel") as HTMLElement;
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  const mod = await import("./sftp");
  sftp = mod.sftp;
  sftpAppliquerVue = mod.sftpAppliquerVue;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  state.sessions.clear();
  state.active = null;
  sftp.open = false;
});

describe("SFTP : le panneau et le bouton suivent le type de l'onglet actif", () => {
  it("grise le bouton et referme le panneau sur un onglet sans système de fichiers (RDP)", () => {
    // Onglet SSH avec panneau ouvert.
    state.sessions.set(1, sessionSsh(1));
    state.active = 1;
    sftp.open = true;
    sftpAppliquerVue();
    expect(toggle().disabled).toBe(false);
    expect(panneau().classList.contains("open")).toBe(true);

    // Bascule sur un onglet RDP : son id n'est pas dans state.sessions.
    state.active = 2;
    sftpAppliquerVue();
    expect(toggle().disabled).toBe(true);
    expect(panneau().classList.contains("open")).toBe(false);
    // La préférence d'ouverture n'est pas perdue, seulement masquée.
    expect(sftp.open).toBe(true);
  });

  it("restaure le panneau au retour sur l'onglet SSH sans double appui", () => {
    state.sessions.set(1, sessionSsh(1));
    state.active = 1;
    sftp.open = true;
    sftpAppliquerVue();

    // Passage RDP puis retour SSH.
    state.active = 2;
    sftpAppliquerVue();
    state.active = 1;
    sftpAppliquerVue();

    expect(toggle().disabled).toBe(false);
    expect(panneau().classList.contains("open")).toBe(true);
    expect(sftp.open).toBe(true);
  });

  it("laisse le bouton grisé et le panneau fermé quand aucun onglet n'a de système de fichiers", () => {
    // Panneau jamais ouvert, onglet RDP seul.
    state.active = 2;
    sftpAppliquerVue();
    expect(toggle().disabled).toBe(true);
    expect(panneau().classList.contains("open")).toBe(false);
  });
});
