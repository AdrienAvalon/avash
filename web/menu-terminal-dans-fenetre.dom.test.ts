// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : le menu contextuel du terminal
// (#term-context, huit entrées) posait brutalement les coordonnées du clic
// (m.style.left = clientX ; m.style.top = clientY) au lieu de passer par
// placerMenu comme les cinq autres menus. Le terminal occupant presque toute la
// fenêtre, un clic droit dans son quart inférieur poussait « Tout sélectionner »,
// « Effacer l'écran » et « Enregistrer la session » hors écran, et un menu de
// 200 px était tronqué près du bord droit. Ce test verrouille que le menu reste
// entièrement dans la fenêtre après un clic droit près du coin bas/droit.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

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
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a pas matchMedia, que theme.ts (importé en cascade) appelle au chargement. */
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
}

let state: typeof import("./etat").state;

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // Importer ./main d'abord fixe l'ordre d'évaluation du cycle main <-> menu-hote.
  await import("./main");
  await import("./terminal-outils");
  state = (await import("./etat")).state;
});

describe("menu contextuel du terminal maintenu dans la fenêtre", () => {
  it("ne déborde pas après un clic droit près du coin bas/droit", () => {
    // Fenêtre 1280x720, menu de 200x300 mesuré par getBoundingClientRect.
    Object.defineProperty(window, "innerWidth", { value: 1280, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 720, configurable: true });
    const menu = document.getElementById("term-context")!;
    menu.getBoundingClientRect = () =>
      ({ width: 200, height: 300, left: 0, top: 0, right: 0, bottom: 0, x: 0, y: 0, toJSON() {} }) as DOMRect;

    // Une session active : le gestionnaire sort tôt si state.active est null.
    // Une Map vide suffit (s?. reste indéfini, hasSel/enCours = false).
    state.active = 1;
    state.sessions.clear();

    // Clic droit à 30 px du bord droit et 20 px du bord bas.
    const terminal = document.getElementById("terminal")!;
    terminal.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 1250, clientY: 700 }),
    );

    const left = parseFloat(menu.style.left);
    const top = parseFloat(menu.style.top);
    // Sans placerMenu, left=1250/top=700 : le menu de 200x300 débordait à droite
    // (1450 > 1280) et en bas (1000 > 720).
    expect(left + 200).toBeLessThanOrEqual(1280);
    expect(top + 300).toBeLessThanOrEqual(720);
    expect(menu.classList.contains("open")).toBe(true);
  });
});
