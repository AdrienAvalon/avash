// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : dans `ws.onclose`, le front appelait
// `rdp_close` (qui retire le journal côté Rust) AVANT `showRdpClosed`, qui lit
// `rdp_diagnostic`. Les deux commandes étant synchrones et exécutées en ligne
// dans l'ordre d'émission, le journal était effacé avant la lecture :
// l'incrustation « Connexion RDP fermée » restait muette. Le correctif montre
// l'incrustation d'abord (sa lecture du diagnostic part donc avant la
// fermeture). Ce test verrouille l'ordre des appels et l'affichage de la raison.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

// On enregistre l'ordre des commandes IPC : c'est lui, et non le contenu, qui
// révèle le défaut (diagnostic lu avant, fermeture après).
const commandes: string[] = [];
const DIAGNOSTIC = "connecté : 10.0.0.1:3389\nError: le serveur a fermé la connexion";
/** La réponse de `rdp_diagnostic` ; un test la retarde pour voir l'ordre. */
let diagnostic: () => Promise<string> = () => Promise.resolve(DIAGNOSTIC);
const invoke = vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>((cmd) => {
  commandes.push(cmd);
  if (cmd === "rdp_open") return Promise.resolve({ port: 1234, token: "jeton" });
  if (cmd === "rdp_diagnostic") return diagnostic();
  return Promise.resolve(undefined);
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: unknown) => invoke(cmd, args) }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn().mockResolvedValue(""), writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
// Collaborateurs d'`openRdp` sans intérêt ici : on les neutralise pour n'exercer
// que le chemin de fermeture (ordre des appels et incrustation).
vi.mock("./main", () => ({ loadHosts: vi.fn(), renderHosts: vi.fn(), rafraichirLignes: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(), closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn() }));
vi.mock("./vue-partagee", () => ({ appliquerVue: vi.fn(), estAffiche: () => false, surFermeture: vi.fn(), surFocus: vi.fn() }));
vi.mock("./verrous", () => ({ currentLocks: () => Promise.resolve(null) }));
vi.mock("./raccourcis", () => ({ orderedTabs: () => [], focusTab: vi.fn(), fermerOnglet: vi.fn() }));
vi.mock("./onglets-restauration", () => ({ majMemoireOnglets: vi.fn() }));
vi.mock("./notifications", () => ({ notify: vi.fn(), notifyErreur: vi.fn() }));
vi.mock("./dossiers", () => ({ openMoveModal: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

// WebSocket contrôlable : `openRdp` en instancie un, le test récupère la
// dernière instance pour déclencher `onclose` lui-même.
class FauxWebSocket {
  static OPEN = 1;
  static derniere: FauxWebSocket | null = null;
  readyState = 1;
  binaryType = "";
  onopen: (() => void) | null = null;
  onmessage: ((e: unknown) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) { FauxWebSocket.derniere = this; }
  send(): void {}
  close(): void { this.readyState = 3; }
}

class FauxResizeObserver {
  observe(): void {}
  disconnect(): void {}
}

let openRdp: (cible: { host: string; port: number | null; user: string; password: string; vnc?: boolean }) => Promise<void>;

beforeEach(async () => {
  commandes.length = 0;
  invoke.mockClear();
  document.body.innerHTML = corpsIndex();
  (globalThis as unknown as { WebSocket: unknown }).WebSocket = FauxWebSocket;
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = FauxResizeObserver;
  // jsdom n'a pas matchMedia, que `openRdp` appelle pour le watcher HiDPI.
  window.matchMedia = ((): MediaQueryList =>
    ({
      matches: false,
      media: "",
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as unknown as MediaQueryList) as typeof window.matchMedia;
  ({ openRdp } = await import("./rdp"));
});

afterEach(() => {
  FauxWebSocket.derniere = null;
  diagnostic = () => Promise.resolve(DIAGNOSTIC);
});

describe("fermeture d'un bureau RDP (ws.onclose)", () => {
  it("lit le diagnostic AVANT de fermer côté Rust", async () => {
    await openRdp({ host: "10.0.0.1", port: 3389, user: "adrien", password: "x" });
    const ws = FauxWebSocket.derniere!;
    expect(ws.onclose).toBeTypeOf("function");

    ws.readyState = 3;
    ws.onclose!();

    // C'est l'ordre qui compte : si `rdp_close` (retrait du journal) part avant
    // `rdp_diagnostic`, ce dernier lit un journal déjà vidé — le défaut d'origine.
    await vi.waitFor(() => expect(commandes).toContain("rdp_close"));
    const iDiag = commandes.indexOf("rdp_diagnostic");
    const iClose = commandes.indexOf("rdp_close");
    expect(iDiag).toBeGreaterThanOrEqual(0);
    expect(iClose).toBeGreaterThanOrEqual(0);
    expect(iDiag).toBeLessThan(iClose);
  });

  it("dévoile la raison de la coupure dans .rdp-closed-diag", async () => {
    await openRdp({ host: "10.0.0.1", port: 3389, user: "adrien", password: "x" });
    const ws = FauxWebSocket.derniere!;
    ws.readyState = 3;
    ws.onclose!();

    const zone = document.querySelector(".rdp-closed-diag") as HTMLElement;
    expect(zone).not.toBeNull();
    // La lecture du diagnostic est asynchrone (relances) : on attend l'affichage.
    await vi.waitFor(() => {
      expect(zone.hidden).toBe(false);
      expect(zone.textContent).toContain("fermé la connexion");
    });
  });
});

describe("raison d'une fin en erreur (audit du 27 novembre 2026, FI-10)", () => {
  // La ligne « Error: … » s'écrit au retour du processus, après la fermeture
  // de la WebSocket, quand le journal porte déjà « connecté » : la première
  // lecture ne relançait pas et `rdp_close`, juste derrière, tuait le
  // processus avant qu'il ne l'écrive.
  it("la_lecture_attend_la_fin_du_processus_et_rdp_close_attend_la_lecture", async () => {
    let repondre: (d: string) => void = () => {};
    diagnostic = () => new Promise((r) => { repondre = r; });
    await openRdp({ host: "10.0.0.1", port: 3389, user: "adrien", password: "x" });
    const ws = FauxWebSocket.derniere!;
    ws.readyState = 3;
    ws.onclose!();
    const lecture = invoke.mock.calls.find((c) => c[0] === "rdp_diagnostic");
    expect(lecture?.[1]).toMatchObject({ attendreFin: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(commandes).not.toContain("rdp_close"); // le processus écrit encore
    repondre(DIAGNOSTIC);
    await vi.waitFor(() => expect(commandes).toContain("rdp_close"));
    const zone = document.querySelector(".rdp-closed-diag") as HTMLElement;
    expect(zone.textContent).toContain("fermé la connexion");
  });
});
