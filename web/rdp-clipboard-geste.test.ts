// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : le presse-papiers du poste partait
// vers un serveur RDP sans le moindre geste dans le bureau distant. `focusRdp`
// (bascule d'onglet : Ctrl+Tab, clic d'onglet) et la minuterie de connexion
// poussaient le message [8] ; or en RDP [8] fait annoncer le format au serveur,
// qui peut alors réclamer aussitôt le texte — un mot de passe fraîchement copié
// partait donc à tout bureau ouvert qu'on ne faisait que traverser. Le focus
// lui-même n'est pas un geste (il se déclenche aussi au retour de la fenêtre).
// `pousseAuGeste` verrouille la décision : en RDP le presse-papiers ne part que
// sur un clic dans le canvas (mousedown), jamais au focus, à la connexion ni à
// la bascule d'onglet. En VNC, [8] n'est que mémorisé par le sidecar (rien ne
// part avant un collage explicite [22]), l'annonce peut donc rester liée au
// focus sans fuite.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// rdp.ts câble des écouteurs et importe le cœur du front à l'évaluation ; on
// remplace ./main (effets de bord lourds) et on monte index.html pour que les
// autres modules trouvent leurs éléments, afin de n'exercer que `pousseAuGeste`.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./main", () => ({
  loadHosts: vi.fn(), renderHosts: vi.fn(), rafraichirLignes: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(),
  closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn(),
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

type Evenement = "focus" | "mousedown" | "connexion" | "bascule";
let pousseAuGeste: (vnc: boolean, evenement: Evenement) => boolean;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  ({ pousseAuGeste } = await import("./rdp"));
});

describe("pousseAuGeste — quand le presse-papiers du poste part vers le bureau distant", () => {
  it("RDP : ne pousse jamais sur le focus, la connexion ni la bascule d'onglet", () => {
    // Le cœur du défaut : ces trois chemins arrivaient sans aucun geste dans le
    // bureau (Ctrl+Tab qui traverse un onglet RDP, minuterie de connexion,
    // retour sur la fenêtre) et faisaient malgré tout annoncer le presse-papiers.
    expect(pousseAuGeste(false, "focus")).toBe(false);
    expect(pousseAuGeste(false, "connexion")).toBe(false);
    expect(pousseAuGeste(false, "bascule")).toBe(false);
  });

  it("RDP : pousse sur un clic dans le canvas (le seul geste réel dans le bureau)", () => {
    expect(pousseAuGeste(false, "mousedown")).toBe(true);
  });

  it("VNC : pousse sur le focus, la connexion et la bascule (le sidecar retient jusqu'au collage explicite, sans fuite)", () => {
    expect(pousseAuGeste(true, "focus")).toBe(true);
    expect(pousseAuGeste(true, "connexion")).toBe(true);
    expect(pousseAuGeste(true, "bascule")).toBe(true);
  });

  it("VNC : pas de poussée redondante sur le mousedown (le clic donne déjà le focus, qui pousse)", () => {
    expect(pousseAuGeste(true, "mousedown")).toBe(false);
  });
});
