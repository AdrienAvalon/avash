// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : pendant le démarrage d'un tunnel
// (tunnels.busy contient son id), seul le bouton « Démarrer/Arrêter » était
// désactivé ; « Modifier » et « Supprimer » restaient cliquables. Supprimer une
// définition en cours d'ouverture laissait un tunnel fantôme qui finissait par
// s'installer et écouter sans aucune ligne pour l'arrêter ; modifier réécrivait
// la définition alors que tunnel_start avait déjà capturé l'ancienne. Ce test
// verrouille que les trois boutons de la ligne sont gelés tant que busy tient
// l'id, y compris après un redessin (le timer rappelle renderTunnels à 1,5 s).
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import type { TunnelDef } from "./filters";

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

let renderTunnels: () => void;
let tunnels: {
  defs: TunnelDef[];
  status: Map<string, unknown>;
  busy: Set<string>;
};

/** Une définition de tunnel minimale, telle que renderTunnels la lit. */
function tunnelDef(id: string): TunnelDef {
  return { id, alias: "srv", kind: "local", bind_port: 8080, target_host: "localhost", target_port: 80, name: "" };
}

function boutons(): { toggle: HTMLButtonElement; edit: HTMLButtonElement; del: HTMLButtonElement } {
  const row = document.querySelector("#tunnel-list .tunnel-row")!;
  return {
    toggle: row.querySelector('[data-act="toggle"]') as HTMLButtonElement,
    edit: row.querySelector('[data-act="edit"]') as HTMLButtonElement,
    del: row.querySelector('[data-act="delete"]') as HTMLButtonElement,
  };
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // ./main fixe l'ordre d'évaluation du cycle des modules (cf. les autres tests
  // DOM) ; on récupère ensuite les exports de ./tunnels.
  await import("./main");
  const mod = await import("./tunnels");
  renderTunnels = mod.renderTunnels;
  tunnels = mod.tunnels;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockResolvedValue([]);
  tunnels.defs = [tunnelDef("t1")];
  tunnels.status = new Map();
  tunnels.busy = new Set();
});

describe("renderTunnels : geler la ligne d'un tunnel en cours de démarrage", () => {
  it("hors busy, les trois boutons de la ligne sont actifs", () => {
    renderTunnels();
    const { toggle, edit, del } = boutons();
    expect(toggle.disabled).toBe(false);
    expect(edit.disabled).toBe(false);
    expect(del.disabled).toBe(false);
  });

  it("busy contenant l'id désactive toggle, Modifier ET Supprimer", () => {
    tunnels.busy.add("t1");
    renderTunnels();
    const { toggle, edit, del } = boutons();
    expect(toggle.disabled).toBe(true);
    expect(edit.disabled).toBe(true);
    expect(del.disabled).toBe(true);
  });

  it("un redessin (timer à 1,5 s) ne réactive pas les boutons tant que busy tient l'id", () => {
    tunnels.busy.add("t1");
    renderTunnels();
    renderTunnels();
    const { toggle, edit, del } = boutons();
    expect(toggle.disabled).toBe(true);
    expect(edit.disabled).toBe(true);
    expect(del.disabled).toBe(true);
  });
});

// Trouvé par l'audit du 7 septembre 2026 : renderTunnels vide #tunnel-list
// (innerHTML = "") et recrée chaque ligne ; le minuteur le rappelle toutes les
// 1,5 s (et tunnelStart/tunnelsRefresh juste après un clic). Un bouton de ligne
// focalisé au clavier était détruit à chaque redessin, le focus retombait sur
// <body> et le piège de focus de la modale renvoyait le Tab suivant en haut :
// impossible d'atteindre une ligne basse sans se dépêcher. On note ligne+action
// avant le vidage pour refocaliser après reconstruction.
describe("renderTunnels : le focus survit à un redessin", () => {
  it("le rendu des tunnels conserve le bouton focalisé", () => {
    tunnels.defs = [tunnelDef("t1"), tunnelDef("t2")];
    renderTunnels();
    // Focaliser « Modifier » de la 2e ligne, comme un utilisateur au clavier.
    const cible = document.querySelector<HTMLButtonElement>('#tunnel-list [data-id="t2"] [data-act="edit"]')!;
    cible.focus();
    expect(document.activeElement).toBe(cible);
    // Le minuteur (1,5 s) rappelle renderTunnels : la ligne est reconstruite.
    renderTunnels();
    const actif = document.activeElement as HTMLElement;
    expect(actif.closest<HTMLElement>("[data-id]")?.dataset.id).toBe("t2");
    expect(actif.dataset.act).toBe("edit");
  });
});
