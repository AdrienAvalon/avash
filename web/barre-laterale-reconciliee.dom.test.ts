// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-front-1) : `renderHosts()` reconstruisait toute la
// barre latérale à chaque changement d'état (session qui passe « live », logo
// d'OS qui arrive, sonde de santé, clic d'onglet) : `innerHTML = ""`, sept
// écouteurs par ligne, et l'état de chaque session relu dans les classes CSS de
// son onglet. Une ouverture de session faisait trois reconstructions ; un
// double-clic pendant l'une d'elles tombait sur un nœud détaché (l'aléa E2E de
// vue-partagee, contourné dans helpers.js). La mise à jour d'état passe
// désormais par `rafraichirLignes`, en place ; `renderHosts` ne sert plus
// qu'aux changements de structure.
//
// Le même montage couvre C-SIL-4 (liste vide par erreur de lecture), C-SIL-3
// (voyant périmé daté), C-front-12 (palette aux flèches), C-front-6 (réglage à
// la palette) et K13 (diagnostic sans chemin choisi par la page).
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { rememberOs, state } from "./etat";
import { osBadge, type Host } from "./filters";

const invoke = vi.hoisted(() => vi.fn());
const save = vi.hoisted(() => vi.fn());
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ save, open: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a ni matchMedia, ni document.fonts, ni requestIdleCallback, ni scrollIntoView. */
function shimsNavigateur(): void {
  window.__AVASH_LANGUE = "fr";
  window.matchMedia = ((): MediaQueryList =>
    ({ matches: false, media: "", addEventListener: () => {}, removeEventListener: () => {} }) as unknown as MediaQueryList) as typeof window.matchMedia;
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: () => Promise.resolve([]), ready: Promise.resolve(), add: () => {} },
  });
  window.requestIdleCallback = ((cb: () => void) => setTimeout(cb, 0)) as typeof window.requestIdleCallback;
  Element.prototype.scrollIntoView = function () {};
}

const hote = (i: number): Host => ({
  alias: `srv-${i}`, hostname: `10.0.${Math.floor(i / 250)}.${i % 250}`, user: "root", port: 22,
  identity_file: null, proxy_jump: null, tags: [], folder: `f${i % 7}`,
});

let renderHosts: () => void;
let rafraichirLignes: (cles?: Iterable<string>) => void;
let loadHosts: () => Promise<void>;
const list = () => document.getElementById("host-list")!;
const ligne = (cle: string) => list().querySelector<HTMLElement>(`[data-cle="${cle}"]`)!;
const toasts = () => document.getElementById("toasts")!.textContent ?? "";

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  const main = await import("./main");
  renderHosts = main.renderHosts;
  rafraichirLignes = main.rafraichirLignes;
  loadHosts = main.loadHosts;
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue([]);
  state.sessions.clear();
  state.sante.clear();
  state.filter = "";
  state.tagFilter = null;
  state.rdpHosts = [];
  state.folders = ["f0", "f1", "f2", "f3", "f4", "f5", "f6"];
  state.hosts = Array.from({ length: 500 }, (_, i) => hote(i));
  renderHosts();
});

describe("barre latérale : rendu réconcilié", () => {
  it("la_mise_a_jour_d_etat_garde_les_memes_noeuds", () => {
    const avant = [...list().children];
    state.sante.set("ssh:srv-42", { etat: "joignable", latence_ms: 3, quand: Date.now() });
    rememberOs("srv-7", { id: "debian", like: [], pretty: "Debian 13" });
    rafraichirLignes();
    const apres = [...list().children];
    expect(apres.length).toBe(avant.length);
    apres.forEach((n, i) => expect(n).toBe(avant[i]));
    expect(ligne("ssh:srv-42").querySelector(".dot")!.classList.contains("up")).toBe(true);
    const ini = ligne("ssh:srv-7").querySelector(".ini")!;
    expect(ini.classList.contains("logo")).toBe(true);
    expect(ini.textContent).toBe(osBadge({ id: "debian", like: [], pretty: "" }).glyph);
  });

  it("la_ligne_focalisee_survit_a_une_mise_a_jour_d_etat", () => {
    const centieme = list().querySelectorAll<HTMLElement>(".host")[100];
    centieme.focus();
    expect(document.activeElement).toBe(centieme);
    state.sante.set(`ssh:${centieme.querySelector(".alias")!.textContent}`, { etat: "injoignable", raison: "x", quand: Date.now() });
    rafraichirLignes();
    // Le même nœud, pas un remplaçant refocalisé par la rustine de renderHosts.
    expect(document.activeElement).toBe(centieme);
    expect(centieme.isConnected).toBe(true);
    expect(centieme.querySelector(".dot")!.classList.contains("down")).toBe(true);
  });

  it("changer_de_filtre_reconstruit_la_liste", () => {
    // Garde du chemin complet (il n'a pas changé) : srv-1, srv-10..19, srv-100..199.
    state.filter = "srv-1";
    renderHosts();
    expect(list().querySelectorAll(".host").length).toBe(111);
    expect(document.getElementById("host-count")!.textContent).toBe("111");
  });

  it("le_rafraichissement_coute_une_fraction_du_rendu", () => {
    // Le coût se compte en éléments créés et insérés, pas en millisecondes : un
    // chronomètre cède sous la charge de la suite complète (vu en intégration),
    // un compte ne varie pas. La durée reste affichée, pour information.
    const creer = vi.spyOn(document, "createElement");
    const insertions = new MutationObserver(() => {});
    insertions.observe(list(), { childList: true, subtree: true });
    const elementsInseres = () => insertions.takeRecords()
      .flatMap((r) => [...r.addedNodes]).filter((n) => n.nodeType === Node.ELEMENT_NODE).length;

    const t0 = performance.now();
    renderHosts();
    const rendu = { crees: creer.mock.calls.length, inseres: elementsInseres(), ms: performance.now() - t0 };
    creer.mockClear();
    const t1 = performance.now();
    rafraichirLignes();
    const maj = { crees: creer.mock.calls.length, inseres: elementsInseres(), ms: performance.now() - t1 };
    insertions.disconnect();
    creer.mockRestore();
    console.warn(`500 hôtes : renderHosts ${rendu.ms.toFixed(1)} ms, ${rendu.crees} éléments créés · rafraichirLignes ${maj.ms.toFixed(1)} ms, ${maj.crees} créé(s)`);
    expect(rendu.crees).toBeGreaterThanOrEqual(500);
    expect(rendu.inseres).toBeGreaterThanOrEqual(500);
    expect(maj.crees).toBe(0);
    expect(maj.inseres).toBe(0);
  });

  it("l_etat_de_session_se_lit_dans_l_etat_pas_dans_le_dom", () => {
    // Un onglet sans pastille `.state` : l'ancienne lecture des classes CSS de
    // l'onglet n'y voyait rien ; le champ `etat` de la session fait foi.
    state.sessions.set(99, { id: 99, alias: "srv-3", etat: "live", closed: false, tab: document.createElement("div") } as never);
    rafraichirLignes(["ssh:srv-3"]);
    expect(ligne("ssh:srv-3").querySelector(".dot")!.classList.contains("live")).toBe(true);
    state.sessions.set(99, { id: 99, alias: "srv-3", etat: "connecting", closed: false, tab: document.createElement("div") } as never);
    rafraichirLignes(["ssh:srv-3"]);
    expect(ligne("ssh:srv-3").querySelector(".dot")!.classList.contains("connecting")).toBe(true);
    state.sessions.delete(99);
    rafraichirLignes(["ssh:srv-3"]);
    expect(ligne("ssh:srv-3").querySelector(".dot")!.className).toBe("dot");
  });
});

// Audit du 12 septembre 2026 (C-SIL-1) : la pastille d'un bureau se calculait
// sur la seule présence d'une session ; coupée par le serveur, la session
// restait dans la table et l'hôte « connecté » dans la barre latérale.
describe("pastille d'un bureau distant", () => {
  it("la_pastille_rdp_suit_l_etat_de_la_session_pas_sa_presence", async () => {
    const { rdpSessions } = await import("./rdp");
    state.rdpHosts = [{ id: "b1", name: "bureau", host: "10.0.0.9", port: 3389, user: "moi", width: 0, height: 0, folder: "" }];
    renderHosts();
    const session = { hostId: "b1", etat: "connecting", tab: document.createElement("div"), canvas: document.createElement("canvas"), ws: null };
    rdpSessions.set(77, session as never);
    const pastille = () => ligne("rdp:b1").querySelector(".dot")!.className;
    rafraichirLignes(["rdp:b1"]);
    expect(pastille()).toBe("dot connecting");
    session.etat = "live";
    rafraichirLignes(["rdp:b1"]);
    expect(pastille()).toBe("dot live");
    session.etat = "closed";
    rafraichirLignes(["rdp:b1"]);
    expect(pastille()).toBe("dot");
    rdpSessions.delete(77);
  });
});

// Audit du 12 septembre 2026 (C-SIL-3).
describe("voyants de santé datés", () => {
  it("un_voyant_de_sonde_perime_est_grise_et_date_dans_son_infobulle", () => {
    state.sante.set("ssh:srv-5", { etat: "joignable", latence_ms: 12, quand: Date.now() - 3 * 86_400_000 });
    state.sante.set("ssh:srv-6", { etat: "joignable", latence_ms: 12, quand: Date.now() });
    rafraichirLignes();
    const vieux = ligne("ssh:srv-5").querySelector<HTMLElement>(".dot")!;
    expect(vieux.className).toBe("dot up stale");
    expect(vieux.title).toContain("il y a 3 jours");
    const frais = ligne("ssh:srv-6").querySelector<HTMLElement>(".dot")!;
    expect(frais.className).toBe("dot up");
    expect(frais.title).toBe("Joignable en 12 ms");
  });
});

// Audit du 12 septembre 2026 (C-SIL-4, contrat K5).
describe("liste vide par erreur de lecture", () => {
  const repondre = (reponses: Record<string, () => Promise<unknown>>) =>
    invoke.mockImplementation((cmd: string) => (reponses[cmd] ?? (() => Promise.resolve([])))());

  it("la_liste_dit_pourquoi_elle_est_vide", async () => {
    state.hosts = [];
    repondre({ list_hosts: () => Promise.reject(new Error("Permission denied (os error 13)")) });
    await loadHosts();
    const texte = list().textContent ?? "";
    expect(texte).toContain("~/.ssh/config illisible");
    expect(texte).toContain("Permission denied");
    expect(texte).not.toContain("Aucun hôte dans");
    expect(toasts()).toContain("Permission denied");
  });

  it("un_rdp_yaml_illisible_se_dit_aussi", async () => {
    repondre({ rdp_hosts: () => Promise.reject(new Error("ligne 3 : indentation")) });
    await loadHosts();
    expect(list().textContent).toContain("ligne 3 : indentation");
    // Relu sans erreur : le bandeau disparaît.
    repondre({ list_hosts: () => Promise.resolve([hote(1)]) });
    await loadHosts();
    expect(list().querySelector(".host-erreur")).toBeNull();
  });
});

describe("palette", () => {
  const ouvrir = () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
  const saisir = (q: string) => {
    const input = document.getElementById("palette-input") as HTMLInputElement;
    input.value = q;
    input.dispatchEvent(new Event("input"));
    return input;
  };

  it("une_fleche_ne_recree_pas_les_options", () => {
    // Audit du 12 septembre 2026 (C-front-12) : chaque flèche reconstruisait
    // toutes les lignes (hôtes et commandes), 6000 nœuds par seconde en
    // répétition clavier sur 200 hôtes.
    ouvrir();
    const input = document.getElementById("palette-input") as HTMLInputElement;
    const avant = [...document.querySelectorAll("#palette-results .item")];
    expect(avant.length).toBeGreaterThan(2);
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    const apres = [...document.querySelectorAll("#palette-results .item")];
    apres.forEach((n, i) => expect(n).toBe(avant[i]));
    expect(apres[1].classList.contains("hl")).toBe(true);
    expect(apres[0].classList.contains("hl")).toBe(false);
    expect(apres[1].getAttribute("aria-selected")).toBe("true");
    expect(apres[0].getAttribute("aria-selected")).toBe("false");
    expect(input.getAttribute("aria-activedescendant")).toBe("palette-item-1");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });

  it("la_palette_propose_de_ne_plus_demander_avant_de_fermer_un_onglet", () => {
    ouvrir();
    saisir("demander");
    const noms = [...document.querySelectorAll("#palette-results .item .name")].map((n) => n.textContent);
    expect(noms).toContain("Ne plus demander avant de fermer un onglet ouvert");
    (document.getElementById("palette-input") as HTMLInputElement)
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });

  it("l_export_du_diagnostic_laisse_le_natif_choisir_le_chemin", async () => {
    // Contrat K13 (audit du 12 septembre 2026) : la page ne choisit plus où
    // écrire ; `diagnostic_exporter` ouvre lui-même « Enregistrer sous » et
    // rend le chemin écrit, ou null si l'utilisateur renonce.
    invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "diagnostic_exporter" ? "/home/moi/avash-diagnostic.txt" : []));
    ouvrir();
    const input = saisir("diagnostic");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => expect(toasts()).toContain("/home/moi/avash-diagnostic.txt"));
    const appels = invoke.mock.calls.filter((c) => c[0] === "diagnostic_exporter");
    expect(appels).toHaveLength(1);
    expect(appels[0]).toHaveLength(1);
    expect(save).not.toHaveBeenCalled();
  });
});
