// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : le redimensionnement natif du bureau
// distant (message [5], Display Control DVC) était perdu quand le canal n'était
// pas encore prêt. `sendResize` posait `resizeInFlight` puis, au filet de 3 s,
// se contentait de remettre le drapeau à faux SANS rejouer la taille : côté
// sidecar, [5] est ignoré en silence tant que `encode_resize` renvoie None (le
// canal n'a pas reçu ses capacités) et aucun [1] ne suit. Un serveur qui impose
// sa taille à la connexion (p. ex. 1280×800 alors que la zone fait 1900×1000)
// sur un lien à latence laissait donc l'utilisateur devant un bureau letterboxé
// jusqu'à ce qu'il redimensionne lui-même la fenêtre après les 3 s.
//
// La décision (faut-il redimensionner, et vers quelle taille bornée) est
// extraite dans `prochainRedimensionnement`, rejouée par `sendResize` au
// déclenchement du filet. Ces tests verrouillent cette décision, dont le point
// clé du correctif : le VNC (RFB) n'a pas de canal Display Control (le sidecar
// n'a aucun bras pour [5] et ne renvoie jamais de [1]) — y poser un drapeau puis
// rejouer au filet tournerait en boucle de 3 s permanente, donc on n'y
// redimensionne pas.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// rdp.ts câble des écouteurs et importe le cœur du front à l'évaluation ; on
// remplace ./main (effets de bord lourds) et on monte index.html pour que les
// autres modules trouvent leurs éléments, afin de n'exercer que
// `prochainRedimensionnement`.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./main", () => ({
  loadHosts: vi.fn(), renderHosts: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(),
  closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn(),
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let prochainRedimensionnement: (
  vnc: boolean,
  affiche: boolean,
  largeurConteneur: number,
  hauteurConteneur: number,
  largeurActuelle: number,
  hauteurActuelle: number,
  dpr?: number,
) => [number, number] | null;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  ({ prochainRedimensionnement } = await import("./rdp"));
});

describe("prochainRedimensionnement — décision d'un redimensionnement du bureau distant", () => {
  it("RDP : rejoue la taille du conteneur quand le serveur impose encore la sienne", () => {
    // Le cœur du défaut : le serveur a répondu 1280×800 (sa taille imposée)
    // alors que la zone Avash fait 1900×1000. Au filet de 3 s, on doit rejouer
    // 1900×1000 — c'est ce rejeu qui manquait, laissant le bureau letterboxé.
    expect(prochainRedimensionnement(false, true, 1900, 1000, 1280, 800)).toEqual([1900, 1000]);
  });

  it("VNC : ne redimensionne jamais (pas de canal Display Control, sinon boucle de 3 s)", () => {
    // Même géométrie qu'au-dessus : en VNC la réponse doit être « rien à faire »,
    // faute de quoi le rejeu au filet reposté [5] toutes les 3 s sans fin (le
    // sidecar VNC ne traite pas [5] et ne renvoie jamais de [1]).
    expect(prochainRedimensionnement(true, true, 1900, 1000, 1280, 800)).toBeNull();
  });

  it("bureau non affiché : rien à faire (un volet caché ne se redimensionne pas)", () => {
    expect(prochainRedimensionnement(false, false, 1900, 1000, 1280, 800)).toBeNull();
  });

  it("écart inférieur à 8 px : négligeable, on ne renégocie pas pour du bruit de glissé", () => {
    expect(prochainRedimensionnement(false, true, 1284, 803, 1280, 800)).toBeNull();
  });

  it("borne la taille (largeur paire, 200..8192) comme la demande initiale", () => {
    // Largeur impaire ramenée au pair inférieur ; au-delà de 8192 on plafonne,
    // en deçà de 200 on relève.
    expect(prochainRedimensionnement(false, true, 1283, 900, 800, 700)).toEqual([1282, 900]);
    expect(prochainRedimensionnement(false, true, 10000, 10000, 1280, 800)).toEqual([8192, 8192]);
    expect(prochainRedimensionnement(false, true, 10, 10, 1280, 800)).toEqual([200, 200]);
  });

  it("HiDPI : même unité des deux côtés, pas de boucle de renégociation", () => {
    // Trouvé par l'audit du 7 septembre 2026 : après CONNECTED, `rdpW`/`rdpH`
    // valent la taille renvoyée par le serveur, en pixels PHYSIQUES (1920×1080).
    // Le conteneur mesure 960×540 px CSS sur un écran à 200 %. Si le calcul ne
    // multipliait pas par le DPR, l'écart valait la moitié de la définition et
    // `sendResize` repartait à chaque `ResizeObserver` — le serveur renégociait
    // sans fin. En pixels physiques des deux côtés, l'écart est nul : rien à faire.
    expect(prochainRedimensionnement(false, true, 960, 540, 1920, 1080, 2)).toBeNull();
  });

  it("HiDPI : demande la définition physique quand le serveur impose la sienne", () => {
    // Même géométrie mais le serveur a imposé 1280×800 : on doit demander la
    // définition physique de la zone (960×540 CSS × DPR 2 = 1920×1080).
    expect(prochainRedimensionnement(false, true, 960, 540, 1280, 800, 2)).toEqual([1920, 1080]);
  });
});
