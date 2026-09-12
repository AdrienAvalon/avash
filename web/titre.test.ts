// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : la barre de titre intégrée
// (`#tb-name`, fenêtre sans décorations) restait figée sur le dernier nom posé
// lors d'une transition d'état de session, pas sur l'onglet réellement actif.
// Trois symptômes, une même cause (le titre n'était pas repeint aux bons
// moments) :
//
//  1. Cliquer un autre onglet passait par `focusSession`/`focusRdp` (donc
//     `appliquerVue`) sans repeindre le titre : la barre gardait « db-1 »
//     après passage sur « web-1 ».
//  2. `setTitlebar` ne lisait que `state.sessions` : un onglet RDP actif
//     affichait « Avash » au lieu du nom du bureau.
//  3. `#tb-name` portait `data-i18n="avash"` : changer de langue en cours de
//     session écrasait « web-1 — Avash » par « Avash ».
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));
vi.mock("./dialogues", () => ({
  askPassword: vi.fn(() => Promise.resolve(null)),
  askConfirm: vi.fn(() => Promise.resolve(true)),
  askText: vi.fn(() => Promise.resolve(null)),
  collerDansTerminal: vi.fn(() => Promise.resolve()),
  MODALES_AU_DESSUS: ["confirm-modal", "ask-modal", "pass-modal"],
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a ni matchMedia, ni document.fonts, ni requestIdleCallback. */
function shimsNavigateur(): void {
  window.__AVASH_LANGUE = "fr";
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

function tbName(): string {
  return (document.getElementById("tb-name") as HTMLElement).textContent ?? "";
}

/** Une session SSH réduite aux champs que lisent `setTitlebar`/`appliquerVue`. */
function sessionFactice(alias: string, closed = false): unknown {
  return { alias, closed, term: { element: null }, fit: { fit() {} } };
}

/** Un onglet RDP réduit à son libellé et à ce que touche `appliquerVue`. */
function rdpFactice(nom: string): unknown {
  const tab = document.createElement("div");
  const label = document.createElement("span");
  label.className = "label";
  label.textContent = nom;
  tab.appendChild(label);
  const canvas = document.createElement("canvas");
  document.createElement("div").appendChild(canvas);
  return { canvas, tab, ws: null };
}

type State = { active: number | null; sessions: Map<number, unknown> };
let state: State;
let rdpSessions: Map<number, unknown>;
let setTitlebar: () => void;
let appliquerVue: () => void;
let setLangue: (l: "fr" | "en") => void;

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  state = (await import("./etat")).state as unknown as State;
  rdpSessions = (await import("./rdp")).rdpSessions as unknown as Map<number, unknown>;
  setTitlebar = (await import("./titre")).setTitlebar;
  appliquerVue = (await import("./vue-partagee")).appliquerVue;
  setLangue = (await import("./i18n")).setLangue;
});

beforeEach(() => {
  state.sessions.clear();
  rdpSessions.clear();
  state.active = null;
  setLangue("fr");
});

describe("barre de titre intégrée", () => {
  it("le titre suit l'onglet actif", () => {
    state.sessions.set(1, sessionFactice("web-1"));
    state.sessions.set(2, sessionFactice("db-1"));

    // `appliquerVue` est le passage commun de `focusSession`/`focusRdp` : c'est
    // lui qui doit repeindre le titre au changement d'onglet.
    state.active = 2;
    appliquerVue();
    expect(tbName()).toBe("db-1 — Avash");

    state.active = 1;
    appliquerVue();
    expect(tbName()).toBe("web-1 — Avash");
  });

  it("le titre nomme un bureau RDP actif", () => {
    rdpSessions.set(3, rdpFactice("srv-rdp"));
    state.active = 3;
    setTitlebar();
    expect(tbName()).toBe("srv-rdp — Avash");
  });

  it("le_titre_ne_nomme_plus_un_bureau_ferme", () => {
    // Audit du 12 septembre 2026 (C-SIL-1) : un bureau coupé par le serveur
    // gardait son nom dans la barre de titre, là où un onglet SSH fermé rend
    // « Avash ». Les deux protocoles se lisent désormais pareil.
    rdpSessions.set(3, { ...(rdpFactice("srv-rdp") as object), etat: "closed" });
    state.active = 3;
    setTitlebar();
    expect(tbName()).toBe("Avash");
  });

  it("le_titre_retire_les_controles_bidi", () => {
    // Audit du 12 septembre 2026 (FS-10).
    state.sessions.set(1, sessionFactice("prod\u202ebd"));
    state.active = 1;
    setTitlebar();
    expect(tbName()).toBe("prodbd — Avash");
  });

  it("changer de langue ne perd pas l'alias", () => {
    state.sessions.set(1, sessionFactice("web-1"));
    state.active = 1;
    setTitlebar();
    expect(tbName()).toBe("web-1 — Avash");

    // Bascule en cours de session : `appliquerLangue` ne doit pas réécrire le
    // titre posé par le code (avant le correctif, `#tb-name` portait
    // `data-i18n="avash"` et retombait sur « Avash »).
    setLangue("en");
    expect(tbName()).toBe("web-1 — Avash");
  });
});
