// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : `focusRdp` envoyait le rafraîchissement
// plein écran ([9]) à CHAQUE activation d'onglet, sans regarder si le canvas
// était déjà affiché. Le sidecar y répond par une trame de l'image entière hors
// cadencement (8,3 Mo en 1080p, 33 Mo en 4K), poussée sur la boucle locale puis
// peinte d'un `putImageData` plein écran — payé pour rien quand l'utilisateur
// clique l'onglet DÉJÀ actif, qui n'a rien perdu.
//
// La décision est extraite dans `doitRafraichir(etaitAffiche, aEteReparente,
// wsOuverte)`. Ces tests verrouillent les quatre combinaisons, dont le point clé
// du correctif : « déjà affiché et non reparenté -> false ». On garde aussi les
// deux vraies causes de perte de contenu qui, elles, doivent rafraîchir : le
// passage de caché à visible, et le reparentage du conteneur (WebKitGTK peut
// vider le backing-store d'un canvas brièvement hors document, ce qui arrive à
// chaque `appliquerVue` en vue partagée alors même que `display` n'a jamais valu
// « none ») — se fier au seul `display` rejouerait le flash noir.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// rdp.ts câble des écouteurs et importe le cœur du front à l'évaluation ; on
// remplace ./main (effets de bord lourds) et on monte index.html pour que les
// autres modules trouvent leurs éléments, afin de n'exercer que `doitRafraichir`.
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

let doitRafraichir: (etaitAffiche: boolean, aEteReparente: boolean, wsOuverte: boolean) => boolean;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  ({ doitRafraichir } = await import("./rdp"));
});

describe("doitRafraichir — faut-il redemander l'image entière ([9]) au focus d'un bureau", () => {
  it("déjà affiché et non reparenté : rien à faire (clic sur l'onglet déjà actif)", () => {
    // Le cœur du défaut : le canvas est déjà à l'écran et son conteneur n'a pas
    // bougé, il n'a rien perdu. C'est ce cas qui renvoyait 33 Mo pour rien.
    expect(doitRafraichir(true, false, true)).toBe(false);
  });

  it("passe de caché à visible : rafraîchir (backing-store d'un canvas display:none)", () => {
    expect(doitRafraichir(false, false, true)).toBe(true);
  });

  it("reparenté en vue partagée : rafraîchir même si le canvas était déjà affiché", () => {
    // `appliquerVue` détruit et recrée les `.volet` : le canvas passe brièvement
    // hors document, où WebKitGTK peut vider son backing-store. Se fier au seul
    // `display` (resté « flex ») supprimerait ce rafraîchissement à tort.
    expect(doitRafraichir(true, true, true)).toBe(true);
  });

  it("caché ET reparenté : rafraîchir", () => {
    expect(doitRafraichir(false, true, true)).toBe(true);
  });

  it("WebSocket pas encore ouverte : rien à envoyer (garde de readyState)", () => {
    // À la fin d'`openRdp`, `focusRdp` s'exécute dans la même tâche que la
    // création de la socket : elle est CONNECTING, donc aucun [9] ne part — le
    // serveur envoie déjà son image initiale.
    expect(doitRafraichir(false, false, false)).toBe(false);
    expect(doitRafraichir(false, true, false)).toBe(false);
    expect(doitRafraichir(true, true, false)).toBe(false);
  });
});
