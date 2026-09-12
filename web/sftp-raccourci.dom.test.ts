// @vitest-environment jsdom
// Audit du 12 septembre 2026 : Ctrl+B ouvrait le panneau SFTP même quand un
// terminal avait le focus, alors que c'est le préfixe de tmux : dans une session
// tmux distante, chaque commande (Ctrl+B puis c, n, %…) basculait aussi le
// panneau. Nouvelle politique, alignée sur main.ts : dans un terminal, Ctrl+B
// appartient au distant ; Ctrl+Maj+B ouvre le panneau partout ; hors terminal,
// Ctrl+B reste le raccourci du panneau.
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

let sftp: { open: boolean };
let sftpAppliquerVue: () => void;
/** La zone de saisie d'un terminal xterm, telle que xterm la place dans `.xterm`. */
let saisieTerminal: HTMLTextAreaElement;

function panneauOuvert(): boolean {
  return document.getElementById("sftp-panel")!.classList.contains("open");
}

function ctrlB(cible: HTMLElement, maj = false): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", { key: maj ? "B" : "b", code: "KeyB", ctrlKey: true, shiftKey: maj, bubbles: true, cancelable: true });
  cible.dispatchEvent(ev);
  return ev;
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  const mod = await import("./sftp");
  sftp = mod.sftp;
  sftpAppliquerVue = mod.sftpAppliquerVue;
  (await import("./i18n")).setLangue("fr");
  const terminal = document.createElement("div");
  terminal.className = "xterm";
  saisieTerminal = document.createElement("textarea");
  terminal.appendChild(saisieTerminal);
  document.body.appendChild(terminal);
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue([]);
  state.sessions.set(1, { id: 1, sftpPath: "/root", serie: false } as unknown as Session);
  state.active = 1;
  sftp.open = false;
  sftpAppliquerVue();
  (document.activeElement as HTMLElement | null)?.blur();
});

describe("raccourci du panneau SFTP", () => {
  it("Ctrl+B ouvre le panneau quand le focus n'est pas dans un terminal", () => {
    const ev = ctrlB(document.body);
    expect(panneauOuvert()).toBe(true);
    expect(ev.defaultPrevented).toBe(true);
  });

  it("Ctrl+B dans un terminal est laissé au distant (préfixe de tmux)", () => {
    saisieTerminal.focus();
    expect(document.activeElement).toBe(saisieTerminal);
    const ev = ctrlB(saisieTerminal);
    expect(panneauOuvert()).toBe(false);
    expect(ev.defaultPrevented).toBe(false);
  });

  it("Ctrl+Maj+B ouvre le panneau même depuis un terminal", () => {
    saisieTerminal.focus();
    const ev = ctrlB(saisieTerminal, true);
    expect(panneauOuvert()).toBe(true);
    expect(ev.defaultPrevented).toBe(true);
  });

  it("Ctrl+Maj+B le referme aussi hors terminal", () => {
    ctrlB(document.body, true);
    expect(panneauOuvert()).toBe(true);
    ctrlB(document.body, true);
    expect(panneauOuvert()).toBe(false);
  });
});
