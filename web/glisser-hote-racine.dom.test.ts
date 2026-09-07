// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : la zone racine (#host-list, câblée
// par dossiers.ts avec hover = false) couvre toute la liste ; un dépôt relâché
// sur une ligne d'hôte (ou sur l'hôte glissé lui-même, faute de
// pointer-events:none) remontait jusqu'à #host-list et renvoyait l'hôte à la
// racine, réécrivant ~/.ssh/config. Ces tests verrouillent que seul un dépôt
// hors de toute ligne range à la racine, et que redéposer un hôte dans son
// propre dossier ne fait rien.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state } from "./etat";
import type { Host } from "./filters";

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

/** Un DragEvent minimal : jsdom n'implémente pas DataTransfer. */
function evenementGlisser(type: string, payload: { kind: string; id: string }): Event {
  const dt = {
    types: ["text/avash-host"],
    dropEffect: "",
    effectAllowed: "move",
    getData: (t: string) => (t === "text/avash-host" ? JSON.stringify(payload) : ""),
    setData: () => {},
  };
  const ev = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "dataTransfer", { value: dt });
  return ev;
}

function hote(alias: string, folder: string): Host {
  return { alias, hostname: `${alias}.local`, user: "root", port: 22, identity_file: null, proxy_jump: null, tags: [], folder };
}

let renderHosts: () => void;
let moveHostTo: (kind: string, id: string, folder: string) => Promise<void>;

const appelsSetFolder = () => invoke.mock.calls.filter((c) => c[0] === "host_set_folder");

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  const main = await import("./main");
  renderHosts = main.renderHosts;
  moveHostTo = main.moveHostTo;
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockResolvedValue([]);
  state.folders = ["prod"];
  state.rdpHosts = [];
  state.hosts = [hote("db-1", "prod"), hote("web-1", "prod")];
  renderHosts();
});

describe("glisser un hôte à la racine", () => {
  it("ne sort pas un hôte de son dossier quand on le relâche sur une ligne d'hôte", () => {
    const list = document.getElementById("host-list")!;
    const lignes = [...list.querySelectorAll<HTMLElement>(".host")];
    // Deux lignes rendues, dans le dossier « prod ».
    expect(lignes.length).toBe(2);
    // On relâche db-1 sur la ligne web-1 (ou sur lui-même : même remontée).
    lignes[0].dispatchEvent(evenementGlisser("drop", { kind: "ssh", id: "db-1" }));
    expect(appelsSetFolder()).toHaveLength(0);
  });

  it("range à la racine un dépôt sur la zone vide de la liste", () => {
    const list = document.getElementById("host-list")!;
    list.dispatchEvent(evenementGlisser("drop", { kind: "ssh", id: "db-1" }));
    const appels = appelsSetFolder();
    expect(appels).toHaveLength(1);
    expect(appels[0][1]).toEqual({ alias: "db-1", folder: "" });
  });
});

describe("moveHostTo", () => {
  it("ne réécrit rien si le dossier cible est déjà celui de l'hôte", async () => {
    await moveHostTo("ssh", "db-1", "prod");
    expect(appelsSetFolder()).toHaveLength(0);
  });
});
