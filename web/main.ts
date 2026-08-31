// Avash front v0.2 — terminal interactif réel : xterm.js ↔ PTY Rust (russh)

import "@xterm/xterm/css/xterm.css";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ic, fileIconName, hydrateIcons } from "./icons";
import { partageClipboard, setPartageClipboard } from "./prefs";
import {
  humanSize, filterHosts, allTags, remoteJoin, parentDir, isPasswordRequired, isHostKeyChanged, stripHtml, hostInitials, hostHue, osBadge,
  sortSftpEntries, shortDate, shellQuote, validFileName, snippetPreview, snippetVars, renderSnippet, type SftpEntry, type Snippet,
  describeTunnel, tunnelFlag, tunnelTraffic, activeTunnelsByHost,
  type Host, type TunnelDef, type TunnelStatus, type TunnelKind, type OsInfo,
  buildFolderTree, folderNodeCount, rdpScancode, le16, rdpMousePos, choisirVerrous, type FolderNode,
} from "./filters";

// ---------- Systeme distant par hote ----------
//
// Detecte a chaque ouverture de session (evenement `host-os`), memorise
// dans localStorage pour afficher le logo des le lancement suivant, avant
// meme de se connecter.

const OS_CACHE_KEY = "avash.os.v1";
const osByHost = new Map<string, OsInfo>();
try {
  const raw = localStorage.getItem(OS_CACHE_KEY);
  if (raw) for (const [k, v] of Object.entries(JSON.parse(raw) as Record<string, OsInfo>)) osByHost.set(k, v);
} catch { /* cache absent ou corrompu : on repart de zero */ }

function rememberOs(label: string, os: OsInfo) {
  osByHost.set(label, os);
  try {
    localStorage.setItem(OS_CACHE_KEY, JSON.stringify(Object.fromEntries(osByHost)));
  } catch { /* stockage indisponible : le logo vivra le temps de la session */ }
}

type Session = {
  id: number;
  alias: string;
  term: Terminal;
  fit: FitAddon;
  tab: HTMLElement;
  search: SearchAddon;
  /** Session terminee cote serveur : le clavier ne part plus au shell. */
  closed: boolean;
  /** Rouvre la meme cible dans ce meme onglet (Entree apres deconnexion). */
  reconnect: (() => Promise<void>) | null;
  /** Dossier distant courant du panneau SFTP, propre a chaque onglet. */
  sftpPath: string;
};

/** Taille de police partagée par tous les terminaux (Ctrl +/−). */
let terminalFontSize = 14;
const FONT_MIN = 9;
const FONT_MAX = 28;

const state = {
  hosts: [] as Host[],
  filter: "",
  nextId: 1,
  active: null as number | null,
  sessions: new Map<number, Session>(),
  /** Hote surligne par un simple clic (sans connexion). */
  pickedAlias: null as string | null,
  /** Bureau RDP surligné par un simple clic (id). */
  pickedRdp: null as string | null,
  /** Filtre par tag actif (null = tous). */
  tagFilter: null as string | null,
  /** Dossiers connus (registre + dérivés des hôtes), triés. */
  folders: [] as string[],
};

/** Dossiers repliés (persisté par machine). */
const collapsedFolders = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem("avash.folders.collapsed") ?? "[]") as string[];
    } catch {
      return [];
    }
  })(),
);
function saveCollapsed() {
  try {
    localStorage.setItem("avash.folders.collapsed", JSON.stringify([...collapsedFolders]));
  } catch {
    /* stockage indispo */
  }
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ⚠️ `fontFamily` n'appartient PAS au theme : c'est une option du Terminal.
// Place ici, il etait purement ignore et xterm.js retombait sur son defaut
// (courier-new), d'ou un rendu tres laid.
const THEME_DARK = {
  background: "#0d0f16",
  foreground: "#dfe3ee",
  cursor: "#8b7cf6",
  cursorAccent: "#0d0f16",
  selectionBackground: "rgba(139, 124, 246, .30)",
  selectionForeground: "#ffffff",
  // Palette ANSI : contrastee sur fond sombre, sans saturation criarde.
  black: "#1b1e29", red: "#ef6b73", green: "#4ad295", yellow: "#e8b765",
  blue: "#6e9df8", magenta: "#b98cf7", cyan: "#5fd0e8", white: "#c8cddb",
  brightBlack: "#59617a", brightRed: "#ff8a91", brightGreen: "#71e6ae",
  brightYellow: "#ffd083", brightBlue: "#97bcff", brightMagenta: "#d5adff",
  brightCyan: "#8ce4f7", brightWhite: "#ffffff",
};

/** Thème clair du terminal : fond clair, ANSI assombris pour rester lisibles. */
const THEME_LIGHT = {
  background: "#f6f7f9",
  foreground: "#1f2430",
  cursor: "#6d5cf0",
  cursorAccent: "#f6f7f9",
  selectionBackground: "rgba(109,92,240,.22)",
  selectionForeground: "#0b0d14",
  black: "#2c3140", red: "#c8353d", green: "#1f9d57", yellow: "#9a6a15",
  blue: "#3059c8", magenta: "#8043c8", cyan: "#0d7d97", white: "#5a6478",
  brightBlack: "#7a8296", brightRed: "#e0555d", brightGreen: "#28b26a",
  brightYellow: "#b3841f", brightBlue: "#4a76e8", brightMagenta: "#9a5fe0",
  brightCyan: "#1596b0", brightWhite: "#1f2430",
};

// --- Thème de l'interface : système (défaut), clair, ou sombre ---

type ThemePref = "system" | "light" | "dark";
const THEME_KEY = "avash.theme";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch { /* stockage indispo */ }
  return "system";
}
let themePref: ThemePref = readThemePref();

/** Sombre effectif, une fois la préférence système résolue. */
function isDark(): boolean {
  return themePref === "dark" || (themePref === "system" && systemDark.matches);
}

function terminalTheme() {
  return isDark() ? THEME_DARK : THEME_LIGHT;
}

/** Applique la préférence : attribut racine, terminaux ouverts, bouton. */
function applyTheme() {
  const root = document.documentElement;
  if (themePref === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", themePref);
  const th = terminalTheme();
  for (const s of state.sessions.values()) s.term.options.theme = th;
  const btn = $("theme-toggle");
  const icon = themePref === "system" ? "monitor" : themePref === "light" ? "sun" : "moon";
  btn.innerHTML = ic(icon);
  btn.title = `Thème : ${themePref === "system" ? "système" : themePref === "light" ? "clair" : "sombre"} (cliquer pour changer)`;
}

function cycleTheme() {
  themePref = themePref === "system" ? "light" : themePref === "light" ? "dark" : "system";
  try { localStorage.setItem(THEME_KEY, themePref); } catch { /* stockage indispo */ }
  applyTheme();
}

// Le système change (nuit/jour) : ne recolorer que si on le suit.
systemDark.addEventListener("change", () => { if (themePref === "system") applyTheme(); });

/**
 * Nerd Font en tete : l'invite d'un shell moderne (fish, starship, powerlevel)
 * utilise des glyphes que les polices classiques ne contiennent pas et
 * remplacent par des carres vides.
 */
const FONT_STACK =
  '"Avash Mono", "MesloLGS Nerd Font Mono", "Hack", "Consolas", ' +
  '"DejaVu Sans Mono", ui-monospace, monospace';


/**
 * Attend que la police embarquee soit chargee.
 *
 * xterm.js mesure la largeur d'un caractere a l'initialisation. Si la police
 * arrive apres, il garde les metriques de la police de repli : colonnes
 * decalees, curseur mal place, cadres semi-graphiques disjoints. On attend
 * donc une fois, au demarrage, avant d'ouvrir le moindre terminal.
 */
let fontReady: Promise<void> | null = null;

function ensureFontLoaded(): Promise<void> {
  if (!fontReady) {
    fontReady = (async () => {
      try {
        await Promise.all([
          document.fonts.load('400 14px "Avash Mono"'),
          document.fonts.load('600 14px "Avash Mono"'),
        ]);
        await document.fonts.ready;
      } catch {
        // Police indisponible : on continue avec la pile de repli plutot
        // que de bloquer l'ouverture d'un terminal.
      }
    })();
  }
  return fontReady;
}

/** Etat agrege des sessions d'un hote, pour la pastille de la liste. */
function hostSessionState(alias: string): "" | "live" | "connecting" {
  let st: "" | "live" | "connecting" = "";
  for (const s of state.sessions.values()) {
    if (s.alias !== alias || s.closed) continue;
    const dot = s.tab.querySelector(".state");
    if (dot?.classList.contains("live")) return "live";
    if (dot?.classList.contains("connecting")) st = "connecting";
  }
  return st;
}

/** Barre de filtres par tag, sous l'en-tete « Hôtes ». */
function renderTagBar() {
  const bar = $("tag-bar");
  const tags = allTags(state.hosts);
  if (tags.length === 0) { bar.hidden = true; return; }
  bar.hidden = false;
  bar.innerHTML = "";
  for (const t of tags) {
    const c = document.createElement("span");
    c.className = "tag-pill" + (t === state.tagFilter ? " on" : "");
    c.textContent = t;
    c.addEventListener("click", () => {
      state.tagFilter = state.tagFilter === t ? null : t;
      renderHosts();
    });
    bar.appendChild(c);
  }
  if (state.tagFilter) {
    const clear = document.createElement("span");
    clear.className = "tag-pill clear";
    clear.textContent = "✕";
    clear.title = "Effacer le filtre";
    clear.addEventListener("click", () => { state.tagFilter = null; renderHosts(); });
    bar.appendChild(clear);
  }
}

// ---------- Arbre des hôtes (dossiers unifiés SSH + RDP) ----------

type TreeItem = { kind: "ssh"; ssh: Host } | { kind: "rdp"; rdp: RdpHostT };
type TreeNode = FolderNode<TreeItem>;

/** Construit l'arbre unifié à partir du registre de dossiers et des hôtes SSH+RDP.
 *  (Logique pure dans filters.ts, testée ; ici on ne fait que l'alimenter.) */
function buildTree(): TreeNode {
  return buildFolderTree<TreeItem>(state.folders, [
    ...state.hosts.map((h) => ({ folder: h.folder ?? "", item: { kind: "ssh", ssh: h } as TreeItem })),
    ...rdpHostsList.map((h) => ({ folder: h.folder ?? "", item: { kind: "rdp", rdp: h } as TreeItem })),
  ]);
}

/** Une ligne d'hôte SSH (avatar, logo distro, tags, état), déplaçable. */
/** Les lignes de la barre latérale, dans l'ordre où on les parcourt.
 *  Hôtes SSH, bureaux RDP et dossiers confondus : c'est ce que voit l'œil. */
function lignesBarre(): HTMLElement[] {
  return [...$("host-list").querySelectorAll<HTMLElement>("[data-cle]")];
}

/** Un seul arrêt de tabulation pour toute la liste (« tabindex glissant »).
 *
 *  Poser `tabindex=0` sur chaque ligne demandait deux cents pressions de Tab
 *  pour traverser une barre latérale bien remplie : le champ de recherche et
 *  les boutons du bas devenaient inatteignables en pratique. On entre donc dans
 *  la liste en un Tab, et on s'y déplace aux flèches. */
function majTabulation(courante?: HTMLElement): void {
  const lignes = lignesBarre();
  if (lignes.length === 0) return;
  const cible =
    courante ??
    lignes.find((l) => l.classList.contains("picked")) ??
    lignes[0];
  for (const l of lignes) l.tabIndex = l === cible ? 0 : -1;
}

/** Rend une ligne de la barre latérale utilisable au clavier.
 *
 *  Ces lignes étaient de simples `div` : la connexion passait par un
 *  double-clic et les options par un clic droit. Tab sautait donc du champ de
 *  recherche aux boutons du bas, la liste entière restant hors d'atteinte — on
 *  ne pouvait ni connecter, ni éditer, ni déplacer, ni supprimer un hôte sans
 *  souris. La palette rattrapait la connexion, rien d'autre.
 *
 *  Entrée agit, Maj+F10 et la touche Menu ouvrent le menu contextuel au bord de
 *  la ligne, les flèches se déplacent, Origine et Fin vont aux extrémités.
 *  `cle` identifie la ligne d'un rendu à l'autre, pour lui rendre le focus. */
function rendreAtteignableAuClavier(
  el: HTMLElement,
  cle: string,
  agir: () => void,
  menu: (position: { clientX: number; clientY: number }) => void,
): void {
  el.dataset.cle = cle;
  el.tabIndex = -1; // majTabulation() désignera l'unique arrêt
  el.setAttribute("role", "button");
  // Le focus vaut sélection : sans cela le cadre bleu restait sur la dernière
  // ligne cliquée pendant que l'anneau de focus était ailleurs, et l'on ne
  // savait plus laquelle des deux allait agir.
  el.addEventListener("focus", () => {
    for (const n of $("host-list").querySelectorAll(".picked")) n.classList.remove("picked");
    el.classList.add("picked");
    majTabulation(el);
  });
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      agir();
      return;
    }
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const r = el.getBoundingClientRect();
      // Le menu s'ouvre au bord de la ligne ; `placerMenu` le recadre s'il
      // dépasse — le dernier hôte de la liste ouvrait sinon un menu dont le bas
      // sortait de la fenêtre.
      menu({ clientX: r.left + 16, clientY: r.bottom - 4 });
      const ouvert = MENUS_CONTEXTUELS.map((id) => $(id)).find((m) => m.classList.contains("open"));
      if (ouvert) ouvrirMenuAuClavier(ouvert, el);
      return;
    }
    const lignes = lignesBarre();
    const i = lignes.indexOf(el);
    const vise =
      e.key === "ArrowDown" ? i + 1
      : e.key === "ArrowUp" ? i - 1
      : e.key === "Home" ? 0
      : e.key === "End" ? lignes.length - 1
      : null;
    if (vise === null) return;
    e.preventDefault();
    lignes[Math.max(0, Math.min(vise, lignes.length - 1))]?.focus();
  });
}

/** Les gestes d'une ligne, souris et clavier. Le suffixe était perdu sur les
 *  hôtes dont l'OS est connu — donc les plus utilisés — et n'a jamais nommé les
 *  gestes clavier, alors que l'infobulle est le seul endroit où les découvrir. */
const GESTES_LIGNE = "double-clic ou Entrée : ouvrir ; clic droit ou Maj+F10 : options";

function sshHostElement(h: Host): HTMLElement {
  const el = document.createElement("div");
  el.className = "host";
  el.style.setProperty("--hue", hostHue(h.alias));
  const target = `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}`;
  el.innerHTML = `<span class="avatar"><span class="ini"></span><span class="dot"></span></span><span class="info"><div class="alias"></div><div class="meta"></div></span>`;
  const os = osByHost.get(h.alias);
  const ini = el.querySelector(".ini") as HTMLElement;
  if (os) {
    const b = osBadge(os);
    ini.textContent = b.glyph;
    ini.className = "ini logo";
    el.style.setProperty("--hue", b.color);
  } else {
    ini.textContent = hostInitials(h.alias);
  }
  el.querySelector(".alias")!.textContent = h.alias;
  el.querySelector(".meta")!.textContent = target;
  // Deux badges étaient stylés depuis toujours mais jamais posés : rien
  // n'indiquait qu'un hôte transite par un bastion ni qu'il porte des tunnels
  // vifs — alors que le compte des tunnels était déjà calculé à chaque tick, et
  // déclenchait même un rendu complet de la liste, pour un badge inexistant.
  const info = el.querySelector(".info")!;
  if (h.proxy_jump) {
    const j = document.createElement("span");
    j.className = "jumptag";
    j.textContent = "rebond";
    j.title = `Passe par ${h.proxy_jump}`;
    info.appendChild(j);
  }
  const vifs = tunnels.byHost.get(h.alias) ?? 0;
  if (vifs > 0) {
    const t = document.createElement("span");
    t.className = "tun";
    t.textContent = String(vifs);
    t.title = `${vifs} tunnel(s) ouvert(s) sur cet hôte`;
    info.appendChild(t);
  }
  const dot = el.querySelector(".dot") as HTMLElement;
  dot.className = "dot " + hostSessionState(h.alias);
  if (h.alias === state.pickedAlias) el.classList.add("picked");
  // L'alias peut être tronqué et les tags ne tiennent pas dans la ligne : les
  // deux se retrouvent ici, où l'on va naturellement chercher le détail.
  const detail = [h.alias, target, os?.pretty, h.tags.length > 0 ? `tags : ${h.tags.join(", ")}` : ""]
    .filter(Boolean)
    .join(" · ");
  el.title = `${detail} — ${GESTES_LIGNE}`;
  el.addEventListener("click", () => {
    state.pickedAlias = h.alias;
    state.pickedRdp = null;
    for (const n of $("host-list").querySelectorAll(".host.picked")) n.classList.remove("picked");
    el.classList.add("picked");
  });
  el.addEventListener("dblclick", () => openSession(h));
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openHostMenu(h, e as MouseEvent);
  });
  rendreAtteignableAuClavier(el, `ssh:${h.alias}`, () => void openSession(h), (p) =>
    openHostMenu(h, p as MouseEvent));
  makeHostDraggable(el, "ssh", h.alias);
  return el;
}

/** Une ligne de bureau RDP enregistré, déplaçable. */
function rdpHostElement(h: RdpHostT): HTMLElement {
  const el = document.createElement("div");
  el.className = "host";
  el.innerHTML = `<span class="avatar rdp"><span class="ini logo"></span><span class="dot"></span></span><span class="info"><div class="alias"></div><div class="meta"></div></span>`;
  (el.querySelector(".ini") as HTMLElement).innerHTML = ic("monitor");
  // Voyant vert : une session RDP est ouverte pour cet hôte.
  const live = [...rdpSessions.values()].some((sess) => sess.hostId === h.id);
  (el.querySelector(".dot") as HTMLElement).className = "dot" + (live ? " live" : "");
  el.querySelector(".alias")!.textContent = h.name;
  el.querySelector(".meta")!.textContent = `${h.user}@${h.host}:${h.port}`;
  el.title = `${h.name} · ${h.user}@${h.host}:${h.port} — ${GESTES_LIGNE}`;
  if (h.id === state.pickedRdp) el.classList.add("picked");
  el.addEventListener("click", () => {
    state.pickedRdp = h.id;
    state.pickedAlias = null;
    for (const n of $("host-list").querySelectorAll(".host.picked")) n.classList.remove("picked");
    el.classList.add("picked");
  });
  el.addEventListener("dblclick", () => connectRdpSaved(h));
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openRdpMenu(h, e as MouseEvent);
  });
  rendreAtteignableAuClavier(el, `rdp:${h.id}`, () => void connectRdpSaved(h), (p) =>
    openRdpMenu(h, p as MouseEvent));
  makeHostDraggable(el, "rdp", h.id);
  return el;
}

function makeHostDraggable(el: HTMLElement, kind: "ssh" | "rdp", id: string) {
  el.draggable = true;
  el.addEventListener("dragstart", (e) => {
    el.classList.add("dragging");
    e.dataTransfer?.setData("text/avash-host", JSON.stringify({ kind, id }));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  });
  el.addEventListener("dragend", () => el.classList.remove("dragging"));
}

/** Déplace un hôte (SSH ou RDP) dans un dossier, puis recharge. */
async function moveHostTo(kind: string, id: string, folder: string) {
  try {
    if (kind === "ssh") await invoke("host_set_folder", { alias: id, folder });
    else await invoke("rdp_host_set_folder", { id, folder });
    await loadHosts();
  } catch (e) {
    notifyErreur(`Déplacement impossible : ${e}`);
  }
}

/** Rend un élément « cible de dépôt » pour ranger un hôte dans `folder`. */
function setupFolderDrop(el: HTMLElement, folder: string, hover = true) {
  el.addEventListener("dragover", (e) => {
    if (!e.dataTransfer?.types.includes("text/avash-host")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    if (hover) el.classList.add("drop-hover");
  });
  el.addEventListener("dragleave", () => el.classList.remove("drop-hover"));
  el.addEventListener("drop", (e) => {
    e.preventDefault();
    e.stopPropagation();
    el.classList.remove("drop-hover");
    const raw = e.dataTransfer?.getData("text/avash-host");
    if (!raw) return;
    try {
      const { kind, id } = JSON.parse(raw) as { kind: string; id: string };
      void moveHostTo(kind, id, folder);
    } catch {
      /* charge utile invalide */
    }
  });
}

/** En-tête de dossier (chevron, icône, nom, compteur) — repliable, cible de dépôt. */
function folderRow(node: TreeNode, depth: number): HTMLElement {
  const row = document.createElement("div");
  row.className = "folder-row";
  row.style.setProperty("--depth", String(depth));
  const collapsed = collapsedFolders.has(node.path);
  row.innerHTML = `<span class="chev">${collapsed ? "▸" : "▾"}</span><span class="fic">${ic("folder")}</span><span class="fname"></span><span class="fcount"></span>`;
  row.querySelector(".fname")!.textContent = node.name;
  row.querySelector(".fcount")!.textContent = String(folderNodeCount(node));
  row.title = `${node.path} — clic ou Entrée : plier/déplier ; clic droit ou Maj+F10 : options ; déposer un hôte pour le ranger`;
  row.addEventListener("click", () => {
    if (collapsedFolders.has(node.path)) collapsedFolders.delete(node.path);
    else collapsedFolders.add(node.path);
    saveCollapsed();
    renderHosts();
  });
  row.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openFolderMenu(node.path, e as MouseEvent);
  });
  rendreAtteignableAuClavier(
    row,
    `dossier:${node.path}`,
    () => {
      if (collapsedFolders.has(node.path)) collapsedFolders.delete(node.path);
      else collapsedFolders.add(node.path);
      saveCollapsed();
      renderHosts();
    },
    (p) => openFolderMenu(node.path, p as MouseEvent),
  );
  row.setAttribute("aria-expanded", String(!collapsed));
  setupFolderDrop(row, node.path);
  return row;
}

function itemName(it: TreeItem): string {
  return it.kind === "ssh" ? it.ssh.alias : it.rdp.name;
}

/** Rend récursivement un nœud : sous-dossiers (triés) puis hôtes. */
function renderNode(node: TreeNode, container: HTMLElement, depth: number) {
  const subs = [...node.children.values()].sort((a, b) => a.name.localeCompare(b.name));
  for (const sub of subs) {
    container.appendChild(folderRow(sub, depth));
    if (!collapsedFolders.has(sub.path)) renderNode(sub, container, depth + 1);
  }
  const items = [...node.items].sort((a, b) => itemName(a).localeCompare(itemName(b)));
  for (const it of items) {
    const el = it.kind === "ssh" ? sshHostElement(it.ssh) : rdpHostElement(it.rdp);
    el.style.setProperty("--depth", String(depth));
    container.appendChild(el);
  }
}

function renderHosts() {
  const list = $("host-list");
  // AVANT le vidage, impérativement : `innerHTML = ""` met scrollHeight à 0 et
  // le moteur borne aussitôt scrollTop, si bien qu'une lecture faite ensuite
  // rend toujours 0. Sans cela la liste saute au sommet à chaque événement sans
  // rapport — un logo d'OS qui arrive, un état de session qui change —
  // pendant qu'on la fait défiler.
  const defilement = list.scrollTop;
  // Même raison pour le focus : la ligne focalisée est sur le point d'être
  // détruite, et le focus retomberait sur <body>. On note laquelle, pour la
  // retrouver après reconstruction.
  const focalisee = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>(
    "#host-list [data-cle]",
  )?.dataset.cle;
  list.innerHTML = "";
  renderTagBar();
  const q = state.filter.trim().toLowerCase();
  const filtering = q !== "" || state.tagFilter !== null;
  const sshShown = filterHosts(state.hosts, state.filter, state.tagFilter);
  // Un bureau RDP ne porte pas de tag : filtrer par tag doit donc les écarter
  // tous. Ils restaient affichés, et le compteur les comptait — le filtre avait
  // l'air cassé alors qu'il faisait son travail sur la moitié de la liste.
  const rdpShown = state.tagFilter !== null ? [] : rdpHostsList.filter(
    (h) => !q || h.name.toLowerCase().includes(q) || h.host.toLowerCase().includes(q) || h.user.toLowerCase().includes(q),
  );
  $("host-count").textContent = String(sshShown.length + rdpShown.length);

  if (filtering) {
    // Recherche/filtre : liste plate, sans dossiers (on cherche, on ne range pas).
    for (const h of sshShown) list.appendChild(sshHostElement(h));
    for (const h of rdpShown) list.appendChild(rdpHostElement(h));
  } else {
    renderNode(buildTree(), list, 0);
  }

  if (state.hosts.length === 0 && rdpHostsList.length === 0) {
    const empty = document.createElement("div");
    empty.className = "host-empty";
    empty.innerHTML =
      `<p>Aucun hôte dans <code>~/.ssh/config</code>.</p>` +
      `<p class="sub">Utilise <strong>Connexion directe</strong> ci-dessous, ` +
      `ou crée une clé puis installe-la sur un serveur.</p>`;
    list.appendChild(empty);
  } else if (filtering && sshShown.length + rdpShown.length === 0) {
    const empty = document.createElement("div");
    empty.className = "host-empty";
    // Filtrer par tag seul affichait « Aucun hôte ne correspond à «  » » : le
    // critère cité doit être celui qui filtre réellement.
    const critere = q !== "" ? state.filter : `tag ${state.tagFilter ?? ""}`;
    empty.innerHTML = `<p>Aucun hôte ne correspond à « ${stripHtml(critere)} ».</p>`;
    list.appendChild(empty);
  }
  // Filtrer par tag écarte tous les bureaux RDP — ils n'en portent pas. Ils
  // disparaissaient de la liste et du compteur sans un mot, ce qui se lit comme
  // une perte de données.
  if (state.tagFilter !== null && rdpHostsList.length > 0) {
    const note = document.createElement("div");
    note.className = "host-empty";
    note.innerHTML =
      `<p class="sub">${rdpHostsList.length} bureau${rdpHostsList.length > 1 ? "x" : ""} RDP ` +
      `masqué${rdpHostsList.length > 1 ? "s" : ""} : les bureaux ne portent pas de tag.</p>`;
    list.appendChild(note);
  }
  list.scrollTop = defilement;
  if (focalisee !== undefined) {
    list.querySelector<HTMLElement>(`[data-cle="${CSS.escape(focalisee)}"]`)?.focus();
  }
  majTabulation();
}


type SessionState = "connecting" | "live" | "closed";

/**
 * Reflete l'etat d'une session sur son onglet.
 *
 * Sans ce retour, l'utilisateur ne distingue pas « ca charge » de
 * « c'est mort » : dans les deux cas le terminal ne bouge pas.
 */
function setSessionState(id: number, st: SessionState) {
  const s = state.sessions.get(id);
  if (!s) return;
  const dot = s.tab.querySelector(".state") as HTMLElement | null;
  if (!dot) return;
  dot.className = `state ${st}`;
  dot.title =
    st === "connecting" ? "Connexion en cours…" : st === "live" ? "Connectée" : "Session terminée";
  s.tab.classList.toggle("dead", st === "closed");
  renderHosts();
  sftpSyncButton();
  setTitlebar();
}

/** Cree l'onglet et le terminal. La connexion elle-meme est faite par l'appelant. */
function newSessionShell(label: string) {
  const id = state.nextId++;
  const term = new Terminal({
    theme: terminalTheme(),
    fontFamily: FONT_STACK,
    // Taille entiere : une valeur fractionnaire donne un rendu flou.
    fontSize: terminalFontSize,
    lineHeight: 1.25,
    letterSpacing: 0,
    fontWeight: "400",
    fontWeightBold: "600",
    cursorBlink: true,
    cursorStyle: "bar",
    cursorWidth: 2,
    scrollback: 10000,
    macOptionIsMeta: true,
    allowProposedApi: true,
    // Marge interieure : du texte colle au bord se lit mal.
    // (xterm n'a pas d'option de padding, on le fait en CSS sur le container.)
    drawBoldTextInBrightColors: false,
    minimumContrastRatio: 1.5,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);

  // Onglet
  const tabs = $("tabs");
  tabs.querySelector(".no-session")?.remove();
  const tab = document.createElement("div");
  tab.className = "tab active";
  tab.innerHTML = `<span class="state" title="Connexion…"></span><span class="label"></span><span class="close">✕</span>`;
  tab.querySelector(".label")!.textContent = label;
  tab.addEventListener("click", () => focusSession(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => {
    e.stopPropagation();
    closeSession(id);
  });
  tabs.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
  tabs.appendChild(tab);

  // Zone terminal : un container par session, hidden si inactif
  $("terminal-empty").style.display = "none";
  const container = document.createElement("div");
  container.className = "xterm-container";
  container.style.position = "absolute";
  // Marge un peu plus genereuse a gauche : le texte colle au bord se lit mal.
  container.style.inset = "10px 8px 6px 14px";
  $("terminal").appendChild(container);
  term.open(container);

  // Rendu GPU : nettement plus net et plus fluide que le rendu DOM.
  // On replie silencieusement si le contexte WebGL est indisponible
  // (machine virtuelle, pilote graphique limite) — mieux vaut un rendu
  // moins beau qu'un terminal qui refuse de s'ouvrir.
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    /* rendu DOM par defaut */
  }

  const search = new SearchAddon();
  term.loadAddon(search);
  // Liens cliquables : ouverts dans le navigateur du systeme, jamais dans la
  // webview (qui a acces a invoke). openUrl passe par l'API Tauri.
  term.loadAddon(
    new WebLinksAddon((_e, uri) => {
      invoke("open_external", { url: uri }).catch(() => {});
    }),
  );

  // Copier/coller — indispensable, et attendu par tout le monde :
  //   Ctrl+Shift+C copie la selection, Ctrl+Shift+V colle.
  // On n'intercepte QUE ces combinaisons ; tout le reste (dont Ctrl+C
  // d'interruption) part au shell distant.
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;
    // Raccourcis de l'application : xterm les écrivait DANS le PTY avant que
    // nos écouteurs ne s'en saisissent. Ctrl+B est le préfixe de tmux et Ctrl+K
    // le kill-line de readline : chaque frappe agissait donc deux fois, à
    // distance et localement. On les retient ici pour que seul l'effet local
    // subsiste.
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey) {
      const k = e.key.toLowerCase();
      if (k === "b" || k === "k") return false;
    }
    const mod = e.ctrlKey && e.shiftKey;
    if (mod && e.code === "KeyC") {
      const sel = term.getSelection();
      if (sel) navigator.clipboard.writeText(sel).catch(() => {});
      return false;
    }
    if (mod && e.code === "KeyV") {
      navigator.clipboard.readText().then(
        (t) => invoke("pty_write", { id, data: t }).catch(() => {}),
        () => {},
      );
      return false;
    }
    if (mod && e.code === "KeyF") {
      openTermSearch(id);
      return false;
    }
    // Ctrl +/- / 0 : zoom de police, comme un navigateur.
    if (e.ctrlKey && (e.code === "Equal" || e.code === "NumpadAdd")) {
      setFontSize(terminalFontSize + 1);
      return false;
    }
    if (e.ctrlKey && (e.code === "Minus" || e.code === "NumpadSubtract")) {
      setFontSize(terminalFontSize - 1);
      return false;
    }
    if (e.ctrlKey && e.code === "Digit0") {
      setFontSize(14);
      return false;
    }
    return true;
  });

  term.onData((data) => {
    // Session terminee : Entree relance la connexion, le reste est ignore
    // (l'ecrire au backend ne ferait qu'afficher « Session inconnue »).
    if (s.closed) {
      if (data === "\r" && s.reconnect) void s.reconnect();
      return;
    }
    invoke("pty_write", { id, data }).catch((e) => term.write(`\r\n⚠️ write: ${e}\r\n`));
  });
  // Le shell distant n'a besoin que de la taille finale : on attend une
  // courte accalmie avant de la lui envoyer (un SIGWINCH par image le
  // ferait redessiner son invite en boucle).
  let resizeTimer = 0;
  term.onResize(({ cols, rows }) => {
    clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(() => {
      invoke("pty_resize", { id, cols, rows }).catch(() => {});
    }, 60);
  });

  const s: Session = { id, alias: label, term, fit, tab, search, closed: false, reconnect: null, sftpPath: "" };
  state.sessions.set(id, s);
  state.active = id;
  // N'afficher que ce terminal, et masquer tout bureau RDP : son conteneur est
  // absolu (inset:0) et déborderait dans la marge du terminal + son indicateur
  // passerait au-dessus. (Le clic d'onglet passe par focusSession qui fait pareil.)
  state.sessions.forEach((other, sid) => {
    (other.term.element?.parentElement as HTMLElement).style.display = sid === id ? "block" : "none";
  });
  for (const r of rdpSessions.values()) {
    (r.canvas.parentElement as HTMLElement).style.display = "none";
    r.tab.classList.remove("active");
  }
  $("terminal-empty").style.display = "none";
  focusTerminal(s);
  return { id, term, session: s };
}

/**
 * Avertit dans le terminal si l'ecoute des evenements a echoue : sans elle,
 * la session s'ouvre mais rien ne s'affichera jamais.
 */
function warnIfDeaf(term: Terminal) {
  if (!ptyListenError) return;
  term.write(
    `\x1b[33m⚠️  Avash ne reçoit pas les événements du terminal.\r\n` +
      `   La session va s'ouvrir mais restera muette.\r\n` +
      `   Détail : ${ptyListenError}\x1b[0m\r\n\r\n`,
  );
}

/** Marqueur pose par le backend quand seul le mot de passe manque. */
/** Ouvre une session sur un hote declare dans ~/.ssh/config. */
async function openSession(h: Host) {
  await ensureFontLoaded();
  const { session } = newSessionShell(h.alias);
  warnIfDeaf(session.term);
  session.reconnect = () => connectByAlias(session, h);
  await connectByAlias(session, h);
}

/**
 * Connecte (ou reconnecte) un onglet existant a un hote de `~/.ssh/config`.
 * Le meme id est reutilise : le backend remplace l'ancienne session.
 */
async function connectByAlias(s: Session, h: Host) {
  const { id, term } = s;
  s.closed = false;
  setSessionState(id, "connecting");
  const label = `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}`;

  // Un hote sans IdentityFile n'a aucun moyen de s'authentifier : autant
  // demander le mot de passe AVANT, plutot que d'echouer puis redemander.
  let password: string | null = null;
  let rememberAsked = false;
  try {
    // `host_needs_password` tient compte du trousseau : si un mot de passe
    // y est deja memorise, on ne redemande rien.
    if (await invoke<boolean>("host_needs_password", { alias: h.alias })) {
      const rep = await askPassword(label);
      if (!rep) return; // annulation : ne pas ouvrir d'onglet mort
      password = rep.password;
      rememberAsked = rep.remember;
    }
  } catch {
    /* on tentera sans, le backend dira ce qui manque */
  }

  term.write(`\x1b[90mConnexion à ${label}…\x1b[0m\r\n`);

  for (let essai = 0; essai < 3; essai++) {
    // L'onglet a pu être fermé pendant qu'on attendait : sans cette garde, la
    // boucle continuait pour un onglet qui n'existe plus — une modale « Mot de
    // passe » s'ouvrait pour lui, et la remplir établissait une vraie session
    // SSH sur le serveur, aussitôt jetée. Les autres branches écrivaient dans
    // un Terminal déjà détruit.
    if (!state.sessions.has(id)) return;
    try {
      await invoke("pty_open", { id, alias: h.alias, password, cols: term.cols, rows: term.rows });
      setSessionState(id, "live");
      // On ne memorise qu'apres une connexion REUSSIE : enregistrer un mot
      // de passe refuse le ferait redemander a chaque fois sans jamais
      // marcher, et l'utilisateur ne saurait pas d'ou vient l'echec.
      if (password && rememberAsked) {
        await invoke("password_save", {
          addr: h.hostname ?? h.alias,
          port: h.port,
          user: h.user ?? null,
          password,
        }).catch((e) => term.write(`\r\n\x1b[33m⚠️ Mémorisation impossible : ${e}\x1b[0m\r\n`));
      }
      return;
    } catch (e) {
      const msg = String(e);
      if (isHostKeyChanged(msg)) {
        const clean = msg.replace("[AVASH_HOST_KEY_CHANGED]", "").trim();
        const ok = await askConfirm(`${clean}\n\nOublier l'ancienne clé et réessayer ? (à ne faire que si le changement est légitime)`);
        if (!ok) {
          markClosed(s, "Connexion annulée : clé d'hôte changée.");
          return;
        }
        try {
          await invoke("known_hosts_forget", { addr: h.hostname ?? h.alias, port: h.port ?? null });
          term.write(`\x1b[33m⚠️ Ancienne clé oubliée. Nouvelle tentative…\x1b[0m\r\n`);
        } catch (fe) {
          markClosed(s, `Impossible d'oublier l'ancienne clé : ${fe}`);
          return;
        }
        continue; // réessayer : TOFU réapprend la nouvelle clé
      }
      if (!isPasswordRequired(msg)) {
        // Fermeture volontaire pendant la connexion : rien à signaler, l'onglet
        // n'existe plus. Le back le marque explicitement.
        if (msg.includes("[AVASH_ANNULE]")) return;
        markClosed(s, `⚠️ Échec de la connexion : ${msg}`);
        return;
      }
      // Mot de passe manquant ou refuse : on redemande, jusqu'a 3 fois.
      const rep = await askPassword(
        label,
        essai === 0 ? undefined : "Mot de passe refusé, nouvelle tentative.",
      );
      if (!rep) {
        markClosed(s, "Connexion annulée.");
        return;
      }
      password = rep.password;
      rememberAsked = rep.remember;
    }
  }
  markClosed(s, "⚠️ Trois tentatives ont échoué.");
}

/**
 * Marque un onglet termine et explique quoi faire : sans cette ligne,
 * l'utilisateur ne sait pas si ca charge encore ni comment relancer.
 */
function markClosed(s: Session, why: string) {
  // L'onglet peut avoir été fermé entre-temps : `term` est alors détruit et
  // l'écriture se perd au mieux.
  if (!state.sessions.has(s.id)) return;
  s.closed = true;
  setSessionState(s.id, "closed");
  s.term.write(
    `\r\n\x1b[31m${why}\x1b[0m\r\n` +
      `\x1b[90m── \x1b[0m\x1b[1mEntrée\x1b[0m\x1b[90m : se reconnecter · \x1b[0m` +
      `\x1b[1mCtrl+W\x1b[0m\x1b[90m : fermer l'onglet ──\x1b[0m\r\n`,
  );
}

/**
 * Ouvre une session sur une adresse saisie a la main.
 *
 * Contrairement au chemin par alias, l'echec doit remonter a l'appelant :
 * le formulaire affiche le message et reste ouvert pour corriger la saisie.
 * On referme donc l'onglet plutot que de laisser une coquille morte.
 */
async function openManualSession(t: ManualTarget) {
  await ensureFontLoaded();
  const { id, term, session } = newSessionShell(`${t.user}@${t.addr}`);
  warnIfDeaf(term);
  try {
    await connectManual(session, t);
  } catch (e) {
    closeSession(id);
    throw e;
  }
  // La cible (mot de passe compris) reste en memoire le temps de l'onglet :
  // la reconnexion ne redemande rien. Un echec s'affiche alors dans le
  // terminal, l'onglet n'a plus de formulaire a qui remonter.
  session.reconnect = async () => {
    try {
      await connectManual(session, t);
    } catch (e) {
      if (String(e).includes("[AVASH_ANNULE]")) return;
      markClosed(session, `⚠️ Échec de la connexion : ${e}`);
    }
  };
}

async function connectManual(s: Session, t: ManualTarget) {
  const { id, term } = s;
  s.closed = false;
  setSessionState(id, "connecting");
  term.write(`\x1b[90mConnexion à ${t.user}@${t.addr}…\x1b[0m\r\n`);
  const label = await invoke<string>("pty_open_manual", {
    id,
    addr: t.addr,
    port: t.port,
    user: t.user,
    password: t.password,
    keyPath: t.key_path,
    cols: term.cols,
    rows: term.rows,
  });
  s.tab.querySelector(".label")!.textContent = label;
  setSessionState(id, "live");
}

function focusTerminal(s: Session) {
  s.term.focus();
  requestAnimationFrame(() => s.fit.fit());
}

function focusSession(id: number) {
  state.active = id;
  state.sessions.forEach((s, sid) => {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    (s.term.element?.parentElement as HTMLElement).style.display = active ? "block" : "none";
    if (active) focusTerminal(s);
  });
  // Masquer les bureaux RDP (conteneurs absolus qui recouvriraient le terminal).
  for (const r of rdpSessions.values()) {
    (r.canvas.parentElement as HTMLElement).style.display = "none";
    r.tab.classList.remove("active");
    marquerVisibilite(r, false);
  }
  $("terminal-empty").style.display = "none";
  const cur = state.sessions.get(id);
  sftpSyncButton();
  if (sftp.open && cur) void sftpOpenAt(cur, cur.sftpPath);
  renderHosts(); // met à jour le surlignage « sélectionné »
}

function closeSession(id: number) {
  const s = state.sessions.get(id);
  if (!s) return;
  invoke("pty_close", { id }).catch(() => {});
  s.term.dispose();
  s.tab.remove();
  (s.term.element?.parentElement)?.remove();
  state.sessions.delete(id);
  if (state.active === id) {
    // Le repli se faisait sur la première session SSH seulement : fermer le
    // dernier onglet SSH pendant qu'un bureau RDP vivait affichait l'écran
    // « Aucune session » par-dessus une session bien vivante. On reprend
    // l'ordre réel des onglets, les deux protocoles confondus.
    const suivant = orderedTabs().find((t) => !(t.kind === "ssh" && t.id === id));
    if (!suivant) {
      state.active = null;
      $("terminal-empty").style.display = "flex";
      // Le panneau SFTP appartient a une session : sans session, il n'a plus
      // rien a montrer. On le ferme en meme temps que la derniere connexion.
      if (sftp.open) {
        sftp.open = false;
        $("sftp-panel").classList.remove("open");
      }
      sftpSyncButton();
      setTitlebar();
    } else {
      focusTab(suivant);
    }
  }
}

// Écoute du flux PTY côté Rust → xterm
type PtyPayload = { id: number; data: string };
void listenPty();
async function listenPty() {
  try {
    await listen<PtyPayload>("pty-output", (ev) => {
      const s = state.sessions.get(ev.payload.id);
      if (s && ev.payload.id === state.active) {
        s.term.write(ev.payload.data);
      } else if (s) {
        // bufferise même si pas actif : le terminal xterm stocke
        s.term.write(ev.payload.data);
      }
    });
    await listen<{ id: number; label: string; os: OsInfo }>("host-os", (ev) => {
      rememberOs(ev.payload.label, ev.payload.os);
      renderHosts();
    });
    await listen<{ id: number }>("pty-closed", (ev) => {
      const s = state.sessions.get(ev.payload.id);
      // Un evenement d'une session deja remplacee (reconnexion) ne doit pas
      // marquer la nouvelle comme morte.
      if (!s || s.closed) return;
      markClosed(s, "── session terminée ──");
    });
  } catch (e) {
    // Un echec ici rend TOUS les terminaux muets : la sortie du serveur
    // n'arrive jamais. Le signaler visiblement plutot que dans une console
    // que personne n'ouvre — c'est ce silence qui a masque l'absence de
    // permissions Tauri (capabilities/default.json).
    ptyListenError = String(e);
    console.warn("Écoute des événements PTY indisponible :", e);
  }
}

/** Renseigne si l'ecoute des evenements a echoue au demarrage. */
let ptyListenError: string | null = null;

/** L'ecran d'accueil s'adapte : sans hote, « double-clic » n'aide personne. */
function refreshEmptyHint() {
  $("empty-hint").textContent =
    state.hosts.length === 0
      ? "Aucun hôte configuré — commence par une connexion directe"
      : "Double-clic sur un hôte pour te connecter";
}

async function loadHosts() {
  // Les trois lectures s'enchaînaient, chacune attendant la précédente : trois
  // allers-retours IPC en série sur le chemin du tout premier affichage, alors
  // qu'elles ne dépendent pas les unes des autres.
  const [hotes, bureaux, dossiers] = await Promise.all([
    invoke<Host[]>("list_hosts").catch((e) => {
      console.warn("Config SSH illisible :", e);
      return state.hosts;
    }),
    invoke<RdpHostT[]>("rdp_hosts").catch(() => [] as RdpHostT[]),
    invoke<string[]>("folders_list").catch(() => [] as string[]),
  ]);
  state.hosts = hotes;
  rdpHostsList = bureaux;
  state.folders = dossiers;
  renderHosts();
  refreshEmptyHint();
}

// Search sidebar
const searchEl = $("search") as HTMLInputElement;
// Un rendu complet par frappe : à 200 hôtes, chaque ligne coûte une analyse
// HTML, quatre requêtes de sélecteur et six écouteurs. On regroupe les frappes
// d'une même rafale sur une image, ce qui reste imperceptible à la saisie.
let rechercheEnAttente: number | undefined;
searchEl.addEventListener("input", () => {
  state.filter = searchEl.value;
  window.clearTimeout(rechercheEnAttente);
  rechercheEnAttente = window.setTimeout(renderHosts, 50);
});

// Palette
const paletteEl = $("palette") as HTMLDivElement;
const paletteInput = $("palette-input") as HTMLInputElement;
function paletteOpen() {
  paletteEl.classList.add("open");
  paletteInput.value = "";
  paletteIndex = 0;
  renderPalette();
  paletteInput.focus();
}
function paletteClose() { paletteEl.classList.remove("open"); }
/** Une entrée de la palette : SSH ou bureau RDP, avec son action d'ouverture. */
type EntreePalette = { nom: string; detail: string; icone: string; ouvrir: () => void };

/** Ligne sélectionnée dans la palette (index dans la liste affichée). */
let paletteIndex = 0;
let paletteEntrees: EntreePalette[] = [];

/** Réglages atteignables à la palette, avant la liste des hôtes.
 *  La palette est déjà le seul point d'entrée entièrement au clavier : y placer
 *  les réglages évite d'ajouter une fenêtre de préférences pour un interrupteur. */
function commandesPalette(): EntreePalette[] {
  const actif = partageClipboard();
  return [
    {
      nom: actif ? "Ne plus partager le presse-papiers avec les bureaux RDP"
                 : "Partager le presse-papiers avec les bureaux RDP",
      detail: actif ? "Actuellement échangé dans les deux sens" : "Actuellement non partagé",
      icone: "copy",
      ouvrir: () => {
        setPartageClipboard(!actif);
        // Les sessions ouvertes doivent suivre : le réglage ne vaudrait sinon
        // qu'à partir de la prochaine connexion.
        for (const s of rdpSessions.values()) if (s.ws) annoncerPartageClip(s.ws);
        notify(actif ? "Le presse-papiers n'est plus échangé avec les bureaux distants."
                     : "Le presse-papiers est échangé avec les bureaux distants.");
      },
    },
  ];
}

function renderPalette() {
  const q = paletteInput.value.toLowerCase();
  const res = $("palette-results");
  res.innerHTML = "";

  // Les deux protocoles, comme dans la barre latérale : un bureau RDP était
  // jusqu'ici introuvable à la palette, alors qu'elle promet « un nom d'hôte ».
  paletteEntrees = [
    ...commandesPalette().filter((c) => !q || c.nom.toLowerCase().includes(q)),
    ...filterHosts(state.hosts, q).map((h) => ({
      nom: h.alias,
      detail: `${h.user ?? "?"}@${h.hostname ?? h.alias}`,
      icone: "terminal",
      ouvrir: () => void openSession(h),
    })),
    ...rdpHostsList
      .filter((h) => !q || h.name.toLowerCase().includes(q) || h.host.toLowerCase().includes(q))
      .map((h) => ({
        nom: h.name,
        detail: `${h.user}@${h.host}:${h.port}`,
        icone: "monitor",
        ouvrir: () => void connectRdpSaved(h),
      })),
  ];

  if (paletteEntrees.length === 0) {
    res.innerHTML = `<div class="empty">Aucun hôte pour « ${stripHtml(q)} »</div>`;
    return;
  }
  paletteIndex = Math.min(paletteIndex, paletteEntrees.length - 1);
  paletteEntrees.forEach((e, i) => {
    const item = document.createElement("div");
    item.className = "item" + (i === paletteIndex ? " hl" : "");
    item.id = `palette-item-${i}`;
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", String(i === paletteIndex));
    item.innerHTML = `<span class="pico">${ic(e.icone)}</span><span class="name"></span><span class="sub"></span>`;
    item.querySelector(".name")!.textContent = e.nom;
    item.querySelector(".sub")!.textContent = e.detail;
    item.addEventListener("click", () => { paletteClose(); e.ouvrir(); });
    res.appendChild(item);
  });
  paletteInput.setAttribute("aria-activedescendant", `palette-item-${paletteIndex}`);
}
paletteInput.addEventListener("input", () => { paletteIndex = 0; renderPalette(); });

// Une palette de commandes qui exige la souris perd sa raison d'être : les
// flèches déplacent la sélection, Entrée ouvre la session.
paletteInput.addEventListener("keydown", (e) => {
  if (e.key === "Escape") { paletteClose(); return; }
  if (paletteEntrees.length === 0) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const pas = e.key === "ArrowDown" ? 1 : -1;
    paletteIndex = (paletteIndex + pas + paletteEntrees.length) % paletteEntrees.length;
    renderPalette();
    $(`palette-item-${paletteIndex}`).scrollIntoView({ block: "nearest" });
  } else if (e.key === "Enter") {
    e.preventDefault();
    const choisie = paletteEntrees[paletteIndex];
    paletteClose();
    choisie?.ouvrir();
  }
});
paletteEl.addEventListener("click", (e) => { if (e.target === paletteEl) paletteClose(); });
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    // Ne pas s'ouvrir par-dessus une boîte de dialogue : la palette prenait le
    // focus à la demande de mot de passe, et la frappe suivante — le mot de
    // passe — partait en clair dans son champ de recherche.
    if (document.querySelector(".modal-backdrop.open")) return;
    e.preventDefault();
    paletteOpen();
  }
});

// Le redimensionnement a la souris envoie des dizaines d'evenements par
// seconde ; ajuster le terminal a chacun (canvas WebGL refait, shell distant
// notifie) figeait l'interface. Un seul ajustement par image suffit.
let resizeRaf = 0;
window.addEventListener("resize", () => {
  if (resizeRaf) return;
  resizeRaf = requestAnimationFrame(() => {
    resizeRaf = 0;
    if (state.active === null) return;
    state.sessions.get(state.active)?.fit.fit();
  });
});

// ===== SFTP =====

const sftp = {
  open: false,
  /** Transfert en cours : on refuse d'en lancer un second en parallele. */
  busy: false,
  /** Entree visee par le menu contextuel. */
  ctx: null as { entry: SftpEntry | null; path: string } | null,
};

function sftpSession(): Session | null {
  return state.active === null ? null : (state.sessions.get(state.active) ?? null);
}

function sftpStatus(msg: string, kind: "" | "ok" | "err" = "") {
  const el = $("sftp-status");
  el.textContent = msg;
  el.className = "sftp-status" + (kind ? ` ${kind}` : "");
}

function sftpProgress(done: number, total: number, label: string) {
  const box = $("sftp-progress");
  box.hidden = false;
  box.classList.toggle("indeterminate", total === 0);
  ($("sftp-bar") as HTMLElement).style.width = total ? `${Math.round((done / total) * 100)}%` : "";
  sftpStatus(total ? `${label} ${humanSize(done)} / ${humanSize(total)}` : `${label} ${humanSize(done)}`);
}
function sftpProgressDone() {
  $("sftp-progress").hidden = true;
}

async function sftpNavigate(path: string) {
  const s = sftpSession();
  if (!s) return;
  s.sftpPath = path;
  ($("sftp-path") as HTMLInputElement).value = path;
  const list = $("sftp-list");
  list.innerHTML = `<div class="sftp-status">Chargement…</div>`;
  try {
    const entries = await invoke<SftpEntry[]>("sftp_list", { id: s.id, path });
    // La reponse peut arriver apres un changement d'onglet.
    if (sftpSession() !== s || s.sftpPath !== path) return;
    list.innerHTML = "";
    const sorted = sortSftpEntries(entries);
    if (path !== "/") {
      const up = document.createElement("div");
      up.className = "sftp-entry dir up";
      up.innerHTML = `<span class="ic">${ic("cornerUpLeft")}</span><span class="nm">..</span><span class="sz"></span>`;
      up.addEventListener("dblclick", () => sftpNavigate(parentDir(path)));
      list.appendChild(up);
    }
    // Chaque entrée coûtait deux analyses HTML — le gabarit, puis l'icône, un
    // SVG de plusieurs nœuds réanalysé alors qu'il n'existe que huit icônes
    // distinctes — plus trois écouteurs, et un appendChild dans la liste vivante.
    // Sur /usr/bin (≈ 4000 entrées) cela figeait le fil principal plusieurs
    // secondes. On clone un gabarit, on clone des icônes préparées, on assemble
    // hors document, et les trois écouteurs sont délégués au conteneur.
    const gabarit = document.createElement("div");
    gabarit.innerHTML = `<span class="ic"></span><span class="nm"></span><span class="sz"></span>`;
    const icones = new Map<string, Node>();
    const icone = (nom: string): Node => {
      let n = icones.get(nom);
      if (!n) {
        const porteur = document.createElement("span");
        porteur.innerHTML = ic(nom);
        n = porteur.firstChild!;
        icones.set(nom, n);
      }
      return n.cloneNode(true);
    };
    const lot = document.createDocumentFragment();
    sorted.forEach((e, i) => {
      const el = gabarit.cloneNode(true) as HTMLElement;
      el.className = "sftp-entry" + (e.is_dir ? " dir" : "");
      el.dataset.i = String(i); // retrouve l'entrée depuis le conteneur
      el.firstChild!.appendChild(icone(fileIconName(e.name, e.is_dir)));
      el.querySelector(".nm")!.textContent = e.name;
      el.querySelector(".sz")!.textContent = e.is_dir ? shortDate(e.modified) : humanSize(e.size);
      el.title = e.is_dir
        ? `${e.name} — modifié ${shortDate(e.modified) || "?"} — double-clic : ouvrir`
        : `${e.name} — ${humanSize(e.size)}, modifié ${shortDate(e.modified) || "?"} — double-clic : télécharger`;
      lot.appendChild(el);
    });
    list.appendChild(lot);

    // Délégation : trois écouteurs pour toute la liste, au lieu de trois par
    // entrée. `sftpDelegue` est réarmé à chaque navigation avec le lot courant.
    sftpDelegue(list, sorted, path);
    sftpStatus(`${entries.length} élément${entries.length > 1 ? "s" : ""}`);
  } catch (e) {
    list.innerHTML = "";
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

/** Branche les trois gestes du panneau SFTP sur le conteneur, une fois.
 *
 *  Les entrées sont retrouvées par leur `data-i` : le lot courant et le chemin
 *  courant sont gardés à part, si bien qu'une navigation n'a pas à rebrancher
 *  quoi que ce soit.
 */
let sftpLot: { entries: SftpEntry[]; path: string } = { entries: [], path: "" };
let sftpDelegueBranche = false;
function sftpDelegue(list: HTMLElement, entries: SftpEntry[], path: string): void {
  sftpLot = { entries, path };
  if (sftpDelegueBranche) return;
  sftpDelegueBranche = true;
  const viser = (ev: Event): { el: HTMLElement; e: SftpEntry } | null => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>(".sftp-entry");
    if (!el || el.classList.contains("up")) return null;
    const e = sftpLot.entries[Number(el.dataset.i)];
    return e ? { el, e } : null;
  };
  list.addEventListener("click", (ev) => {
    const cible = viser(ev);
    if (!cible) return;
    for (const n of list.querySelectorAll(".sftp-entry.sel")) n.classList.remove("sel");
    cible.el.classList.add("sel");
  });
  list.addEventListener("dblclick", (ev) => {
    const cible = viser(ev);
    if (!cible) return;
    const { e } = cible;
    if (e.is_dir) void sftpNavigate(remoteJoin(sftpLot.path, e.name));
    else void sftpDownload(remoteJoin(sftpLot.path, e.name), e.name);
  });
  list.addEventListener("contextmenu", (ev) => {
    const cible = viser(ev);
    if (!cible) return;
    ev.preventDefault();
    sftpOpenMenu(cible.e, sftpLot.path, ev as MouseEvent);
  });
}

function sftpRefresh() {
  const s = sftpSession();
  if (s) void sftpNavigate(s.sftpPath || ".");
}

function sftpToggle(force?: boolean) {
  const s = sftpSession();
  if (!s) return;
  sftp.open = force ?? !sftp.open;
  $("sftp-panel").classList.toggle("open", sftp.open);
  $("sftp-toggle").classList.toggle("active", sftp.open);
  if (sftp.open) void sftpOpenAt(s, s.sftpPath);
}

/** Reflete l'etat du bouton SFTP selon l'onglet courant. */
function sftpSyncButton() {
  const has = sftpSession() !== null;
  ($("sftp-toggle") as HTMLButtonElement).disabled = !has;
  $("sftp-toggle").classList.toggle("active", has && sftp.open);
}

/**
 * Ouvre le panneau sur un dossier de depart : on resout d'abord "." en
 * chemin absolu (certains serveurs refusent read_dir(".")), puis on liste.
 */
async function sftpOpenAt(s: Session, path: string) {
  const start = path && path !== "." ? path : "";
  if (start) { void sftpNavigate(start); return; }
  try {
    const home = await invoke<string>("sftp_realpath", { id: s.id, path: "." });
    if (sftpSession() === s) void sftpNavigate(home || ".");
  } catch {
    void sftpNavigate(".");
  }
}
// Le panneau prend sa place par une transition : le terminal ne recoit pas
// d'evenement resize et resterait coupe a droite. On l'ajuste a la fin.
$("sftp-panel").addEventListener("transitionend", (e) => {
  if (e.propertyName === "width") sftpSession()?.fit.fit();
});

async function sftpDownload(remote: string, name: string) {
  const s = sftpSession();
  if (!s) return;
  if (sftp.busy) {
    sftpStatus("Un transfert est déjà en cours.", "err");
    return;
  }
  sftp.busy = true;
  sftpProgress(0, 0, `⬇︎ ${name}`);
  try {
    const res = await invoke<string>("sftp_download", { id: s.id, remote });
    sftpStatus(`✅ ${res}`, "ok");
  } catch (err) {
    sftpStatus(`⚠️ ${err}`, "err");
  } finally {
    sftp.busy = false;
    sftpProgressDone();
  }
}

/** Envoie des fichiers locaux (chemins absolus) dans le dossier courant. */
async function sftpUploadPaths(paths: string[]) {
  const s = sftpSession();
  if (!s || paths.length === 0) return;
  if (sftp.busy) {
    sftpStatus("Un transfert est déjà en cours.", "err");
    return;
  }
  sftp.busy = true;
  const dir = s.sftpPath || ".";
  let ok = 0;
  const errors: string[] = [];
  try {
    for (const local of paths) {
      const name = local.split(/[\\/]/).pop() ?? local;
      sftpProgress(0, 0, `⬆︎ ${name}`);
      try {
        await invoke<string>("sftp_upload", { id: s.id, local, remoteDir: dir });
        ok++;
      } catch (e) {
        errors.push(`${name} : ${e}`);
      }
    }
  } finally {
    sftp.busy = false;
    sftpProgressDone();
  }
  if (errors.length === 0) sftpStatus(`✅ ${ok} fichier${ok > 1 ? "s" : ""} envoyé${ok > 1 ? "s" : ""}`, "ok");
  else sftpStatus(`⚠️ ${errors.join(" · ")}`, "err");
  if (sftpSession() === s) void sftpNavigate(dir);
}

async function sftpPickAndUpload() {
  if (!sftpSession()) return;
  let picked: string[] | string | null;
  try {
    picked = await openDialog({ multiple: true, directory: false, title: "Fichiers à envoyer" });
  } catch (e) {
    sftpStatus(`⚠️ Sélecteur indisponible : ${e}`, "err");
    return;
  }
  if (!picked) return;
  await sftpUploadPaths(Array.isArray(picked) ? picked : [picked]);
}

async function sftpMkdir(dir: string) {
  const s = sftpSession();
  if (!s) return;
  const name = await askText("Nouveau dossier", "Nom du dossier", "");
  if (name === null) return;
  if (!validFileName(name)) {
    sftpStatus("Nom de dossier invalide.", "err");
    return;
  }
  try {
    await invoke("sftp_mkdir", { id: s.id, path: remoteJoin(dir, name) });
    void sftpNavigate(dir);
  } catch (e) {
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

async function sftpRename(entry: SftpEntry, dir: string) {
  const s = sftpSession();
  if (!s) return;
  const name = await askText("Renommer", "Nouveau nom", entry.name);
  if (name === null || name === entry.name) return;
  if (!validFileName(name)) {
    sftpStatus("Nom invalide.", "err");
    return;
  }
  try {
    await invoke("sftp_rename", { id: s.id, from: remoteJoin(dir, entry.name), to: remoteJoin(dir, name) });
    void sftpNavigate(dir);
  } catch (e) {
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

async function sftpDelete(entry: SftpEntry, dir: string) {
  const s = sftpSession();
  if (!s) return;
  const what = entry.is_dir ? `le dossier « ${entry.name} » (doit être vide)` : `« ${entry.name} »`;
  if (!(await askConfirm(`Supprimer ${what} sur le serveur ?\n\nCette action est définitive.`))) return;
  try {
    await invoke("sftp_remove", { id: s.id, path: remoteJoin(dir, entry.name), isDir: entry.is_dir });
    void sftpNavigate(dir);
  } catch (e) {
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

// ----- Menu contextuel du panneau -----

function sftpOpenMenu(entry: SftpEntry | null, path: string, e: MouseEvent) {
  const m = $("sftp-context");
  sftp.ctx = { entry, path };
  // Sans entree (clic dans le vide) : seules les actions de dossier.
  for (const item of m.querySelectorAll<HTMLElement>("[data-act]")) {
    const act = item.dataset.act!;
    const needsEntry = ["download", "rename", "delete", "copy"].includes(act);
    const dirOnly = act === "cd";
    item.hidden = (needsEntry && !entry) || (act === "download" && !!entry?.is_dir) || (dirOnly && !!entry && !entry.is_dir);
  }
  placerMenu(m, e);
  m.classList.add("open");
}
function sftpHideMenu() { $("sftp-context").classList.remove("open"); }
window.addEventListener("click", sftpHideMenu);
window.addEventListener("blur", sftpHideMenu);

$("sftp-context").addEventListener("click", (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const ctx = sftp.ctx;
  sftpHideMenu();
  if (!act || !ctx) return;
  const s = sftpSession();
  if (!s) return;
  const { entry, path } = ctx;
  const full = entry ? remoteJoin(path, entry.name) : path;
  if (act === "download" && entry && !entry.is_dir) void sftpDownload(full, entry.name);
  else if (act === "cd") {
    const target = entry?.is_dir ? full : path;
    invoke("pty_write", { id: s.id, data: `cd ${shellQuote(target)}\r` }).catch(() => {});
    s.term.focus();
  } else if (act === "copy") {
    navigator.clipboard.writeText(full).then(() => sftpStatus(`Chemin copié : ${full}`, "ok"), () => {});
  } else if (act === "rename" && entry) void sftpRename(entry, path);
  else if (act === "mkdir") void sftpMkdir(path);
  else if (act === "delete" && entry) void sftpDelete(entry, path);
});

$("sftp-list").addEventListener("contextmenu", (e) => {
  if ((e.target as HTMLElement).closest(".sftp-entry")) return;
  e.preventDefault();
  const s = sftpSession();
  if (s) sftpOpenMenu(null, s.sftpPath || ".", e as MouseEvent);
});

// ----- Barre du panneau -----

$("sftp-toggle").addEventListener("click", () => sftpToggle());
$("sftp-refresh-btn").addEventListener("click", sftpRefresh);
$("sftp-up").addEventListener("click", () => {
  const s = sftpSession();
  if (s && s.sftpPath !== "/") void sftpNavigate(parentDir(s.sftpPath || "."));
});
$("sftp-up-btn").addEventListener("click", sftpPickAndUpload);
$("sftp-mkdir-btn").addEventListener("click", () => {
  const s = sftpSession();
  if (s) void sftpMkdir(s.sftpPath || ".");
});
$("sftp-path").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    const v = ($("sftp-path") as HTMLInputElement).value.trim();
    if (v) void sftpNavigate(v);
  } else if (e.key === "Escape") {
    ($("sftp-path") as HTMLInputElement).value = sftpSession()?.sftpPath ?? "";
    ($("sftp-path") as HTMLInputElement).blur();
  }
});
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
    e.preventDefault();
    sftpToggle();
  }
});

// ----- Progression des transferts -----

listen<{ id: number; name: string; kind: string; done: number; total: number }>("sftp-progress", (ev) => {
  if (ev.payload.id !== state.active) return;
  sftpProgress(ev.payload.done, ev.payload.total, `${ev.payload.kind === "upload" ? "⬆︎" : "⬇︎"} ${ev.payload.name}`);
}).catch(() => {});

// ----- Glisser-deposer depuis le bureau -----
//
// Tauri livre les chemins des fichiers deposes sur la fenetre. Le panneau
// s'ouvre de lui-meme si besoin : deposer un fichier dit assez clairement
// ce qu'on veut.
getCurrentWebview()
  .onDragDropEvent((ev) => {
    const panel = $("sftp-panel");
    const t = ev.payload.type;
    if (t === "enter" || t === "over") {
      if (!sftpSession()) return;
      if (!sftp.open) sftpToggle();
      panel.classList.add("dragging");
    } else if (t === "leave") {
      panel.classList.remove("dragging");
    } else if (t === "drop") {
      panel.classList.remove("dragging");
      void sftpUploadPaths(ev.payload.paths);
    }
  })
  .catch(() => { /* hors Tauri (tests) : pas de glisser-deposer */ });

// ---------- Saisie d'un texte (nom de fichier, de dossier) ----------

let askResolve: ((v: string | null) => void) | null = null;

function askText(title: string, label: string, initial: string): Promise<string | null> {
  $("ask-title").textContent = title;
  $("ask-label").textContent = label;
  const input = $("ask-input") as HTMLInputElement;
  input.value = initial;
  $("ask-error").hidden = true;
  $("ask-modal").classList.add("open");
  setTimeout(() => {
    input.focus();
    // Renommer : on selectionne le nom sans l'extension.
    const dot = initial.lastIndexOf(".");
    input.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, 30);
  // Une demande déjà en attente doit être close, sinon son résolveur est
  // écrasé et sa promesse n'est jamais tenue : l'appelant reste bloqué à
  // jamais — un onglet figé sur « Connexion en cours… », une closure qui ne se
  // libère pas. Deux double-clics rapides suffisaient.
  askResolve?.(null);
  return new Promise((resolve) => { askResolve = resolve; });
}
function askClose(v: string | null) {
  $("ask-modal").classList.remove("open");
  const r = askResolve;
  askResolve = null;
  r?.(v);
}
$("ask-form").addEventListener("submit", (e) => {
  e.preventDefault();
  askClose(($("ask-input") as HTMLInputElement).value.trim());
});
$("ask-cancel").addEventListener("click", () => askClose(null));
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !$("ask-modal").classList.contains("open")) return;
  e.stopImmediatePropagation(); // voir la note du gestionnaire de confirmation
  askClose(null);
});

// Confirmation maison. La fonction native du navigateur est INOPÉRANTE sous
// WebKitGTK/WRY : elle ne bloque plus et renvoie une Promise (toujours vraie),
// si bien que le test « si l'utilisateur refuse » ne s'arrêtait jamais et que
// les suppressions passaient sans confirmation. Cette modale, elle, attend
// vraiment le choix de l'utilisateur (comme askText remplace prompt).
// Convention : la 1re ligne du texte est le titre, le reste le détail.
let confirmResolve: ((v: boolean) => void) | null = null;
function askConfirm(text: string, opts: { ok?: string; danger?: boolean } = {}): Promise<boolean> {
  const [title, ...rest] = text.split("\n\n");
  $("confirm-title").textContent = title;
  const msg = $("confirm-message");
  msg.textContent = rest.join("\n\n");
  msg.hidden = rest.length === 0;
  const okBtn = $("confirm-ok") as HTMLButtonElement;
  okBtn.textContent = opts.ok ?? "Confirmer";
  // Rouge par défaut : la plupart des confirmations gardent une action destructive.
  const dangereux = opts.danger !== false;
  okBtn.classList.toggle("btn-danger", dangereux);
  $("confirm-modal").classList.add("open");
  // Une confirmation destructive ne pré-focalise pas son bouton rouge : Entrée
  // par réflexe, ou restée enfoncée depuis l'action précédente, supprimait un
  // hôte de ~/.ssh/config avant qu'on ait lu la phrase d'avertissement.
  setTimeout(() => (dangereux ? ($("confirm-cancel") as HTMLButtonElement) : okBtn).focus(), 30);
  confirmResolve?.(false); // cf. askText : ne jamais abandonner une promesse
  return new Promise((resolve) => { confirmResolve = resolve; });
}
function confirmClose(v: boolean) {
  $("confirm-modal").classList.remove("open");
  const r = confirmResolve;
  confirmResolve = null;
  r?.(v);
}
$("confirm-ok").addEventListener("click", () => confirmClose(true));
$("confirm-cancel").addEventListener("click", () => confirmClose(false));
window.addEventListener("keydown", (e) => {
  if (!$("confirm-modal").classList.contains("open")) return;
  if (e.key !== "Escape" && e.key !== "Enter") return;
  // La touche s'arrête ici. Sans cela, le gestionnaire d'Échap déclaré plus bas
  // s'exécutait aussi — et comme cette boîte venait de se refermer, il fermait
  // la fenêtre du dessous : renoncer à une suppression faisait disparaître la
  // fenêtre Tunnels ou Snippets d'où l'on venait.
  e.stopImmediatePropagation();
  e.preventDefault();
  // Entrée suit le bouton focalisé au lieu de valider d'office : sur une
  // confirmation destructive, le focus est sur « Annuler ».
  confirmClose(e.key === "Enter" && document.activeElement === $("confirm-ok"));
});

hydrateIcons();
$("theme-toggle").addEventListener("click", cycleTheme);
applyTheme();
void setupWindowControls();

void loadHosts();
// Prechargement : au moment du clic, la police est deja prete.
void ensureFontLoaded();

// ---------- Verrous clavier (Num / Maj / Défilement) ----------
// Un bureau RDP démarre avec ses propres verrous : si le pavé numérique est
// allumé sur le poste mais éteint dans la session distante, l'utilisateur doit
// appuyer sur Verr.Num pour les réaligner. On suit donc l'état local en
// permanence, pour pouvoir l'imposer au distant dès la connexion.
//
// Le navigateur ne révèle cet état que sur un événement clavier : on l'écoute
// dans toute l'application (en capture), pas seulement sur le canvas RDP — ainsi
// l'état est le plus souvent déjà connu au moment où une session s'ouvre.
let verrousDesEvenements: number | null = null;

/** Bits attendus par le message [10] : 1 = numérique, 2 = majuscules, 4 = défilement. */
function readLocks(e: KeyboardEvent): number {
  return (
    (e.getModifierState("NumLock") ? 1 : 0) |
    (e.getModifierState("CapsLock") ? 2 : 0) |
    (e.getModifierState("ScrollLock") ? 4 : 0)
  );
}
for (const type of ["keydown", "keyup"] as const) {
  window.addEventListener(type, (e) => { verrousDesEvenements = readLocks(e); }, true);
}

/**
 * État des verrous à transmettre au bureau distant, ou `null` si inconnu.
 *
 * Le système est interrogé en premier : une session s'ouvre le plus souvent à
 * la souris, sans qu'aucune touche n'ait été frappée. Les événements clavier ne
 * sont qu'un secours, jamais prioritaires — voir `choisirVerrous`.
 */
async function currentLocks(): Promise<number | null> {
  const duSysteme = await invoke<number | null>("keyboard_locks").catch(() => null);
  return choisirVerrous(duSysteme ?? null, verrousDesEvenements);
}

// ---------- Notifications ----------
//
// Remplace `notifyErreur()`, qui sous WebKitGTK/WRY ne bloque pas et n'affiche pas
// nécessairement quoi que ce soit — la même famille de piège que `confirm()`
// et `prompt()`. Un bandeau n'interrompt rien : l'utilisateur lit l'erreur
// sans perdre ce qu'il était en train de faire, et peut la faire disparaître.

/** Nature du message : change la couleur du liseré et l'insistance annoncée. */
type NatureAvis = "info" | "erreur" | "succes";

/**
 * Affiche un bandeau temporaire en bas à droite.
 *
 * Les erreurs restent plus longtemps et sont annoncées de façon assertive aux
 * lecteurs d'écran : ce sont elles qu'il ne faut pas manquer.
 */
/** Raccourci : tous les anciens `alert()` signalaient un échec. */
function notifyErreur(message: string): void {
  notify(message, "erreur");
}

function notify(message: string, nature: NatureAvis = "info"): void {
  const zone = $("toasts");
  zone.setAttribute("aria-live", nature === "erreur" ? "assertive" : "polite");
  const el = document.createElement("div");
  el.className = `toast ${nature}`;
  el.setAttribute("role", nature === "erreur" ? "alert" : "status");
  const titre = document.createElement("span");
  titre.className = "titre";
  titre.textContent = nature === "erreur" ? "Échec" : nature === "succes" ? "Fait" : "Information";
  const corps = document.createElement("span");
  corps.textContent = message; // textContent : le message peut venir d'un serveur
  const aide = document.createElement("span");
  aide.className = "fermer";
  aide.textContent = "Cliquer pour fermer";
  el.append(titre, corps, aide);

  const retirer = () => {
    el.remove();
    if (zone.children.length === 0) zone.setAttribute("aria-live", "polite");
  };
  el.addEventListener("click", retirer);
  zone.appendChild(el);
  window.setTimeout(retirer, nature === "erreur" ? 9000 : 4500);
}

// ---------- Accessibilité des boîtes de dialogue ----------
// Les modales s'ouvrent en posant la classe « open », depuis une quarantaine
// d'endroits. Plutôt que d'instrumenter chaque appel, on observe la classe :
//  - à l'ouverture, on mémorise l'élément qui avait le focus ;
//  - à la fermeture, on le lui rend (sinon le focus retombe sur le <body> et la
//    navigation au clavier repart du début de la page) ;
//  - tant qu'une modale est ouverte, Tab et Maj+Tab bouclent à l'intérieur : la
//    page derrière est masquée visuellement mais reste sinon atteignable.
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]),' +
  ' textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Éléments réellement atteignables (on écarte ceux que le CSS masque). */
function focusablesIn(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => el.offsetParent !== null || el === document.activeElement,
  );
}

/** La boîte de dialogue actuellement ouverte, s'il y en a une. */
/// Boîtes qui s'ouvrent systématiquement par-dessus une autre.
const MODALES_AU_DESSUS = ["confirm-modal", "ask-modal", "pass-modal"] as const;

function openDialogBox(): HTMLElement | null {
  // Ces trois-là priment : `querySelector` rendait la PREMIÈRE du document, or
  // « tunnels » et « snippets » y précèdent « confirmation ». Le piège de focus
  // enfermait donc Tab dans le formulaire resté derrière la confirmation.
  for (const id of MODALES_AU_DESSUS) {
    const el = document.getElementById(id);
    if (el?.classList.contains("open")) {
      return el.querySelector<HTMLElement>('[role="dialog"]') ?? el;
    }
  }
  const ouvertes = [
    ...document.querySelectorAll<HTMLElement>(".modal-backdrop.open, .palette-backdrop.open"),
  ];
  const back = ouvertes.at(-1) ?? null;
  return back ? (back.querySelector<HTMLElement>('[role="dialog"]') ?? back) : null;
}

window.addEventListener(
  "keydown",
  (e) => {
    if (e.key !== "Tab") return;
    const box = openDialogBox();
    if (!box) return;
    const items = focusablesIn(box);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const cur = document.activeElement as HTMLElement | null;
    const inside = cur ? box.contains(cur) : false;
    if (e.shiftKey && (cur === first || !inside)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (cur === last || !inside)) {
      e.preventDefault();
      first.focus();
    }
  },
  true, // en capture : on passe avant les raccourcis des champs
);

// Le déclencheur ne peut pas être lu au moment de l'ouverture : le code qui
// ouvre une modale y place aussitôt le focus (champ de saisie), et l'observateur
// ci-dessous ne s'exécute qu'ensuite (microtâche). On garde donc en permanence
// le dernier élément focalisé HORS dialogue : c'est lui, le déclencheur.
let lastOutsideFocus: HTMLElement | null = null;
window.addEventListener("focusin", (e) => {
  const el = e.target as HTMLElement | null;
  if (el && !el.closest(".modal-backdrop, .palette-backdrop")) lastOutsideFocus = el;
});

for (const back of document.querySelectorAll<HTMLElement>(".modal-backdrop, .palette-backdrop")) {
  let opener: HTMLElement | null = null;
  new MutationObserver(() => {
    const isOpen = back.classList.contains("open");
    if (isOpen) {
      opener ??= lastOutsideFocus;
    } else if (opener) {
      // Le focus était dans la modale qui vient de se fermer : on le rend au
      // déclencheur, s'il est encore dans le document.
      if (opener.isConnected) opener.focus();
      opener = null;
    }
  }).observe(back, { attributes: true, attributeFilter: ["class"] });
}

// Version affichee (barre laterale + pied) : lue depuis l'app, jamais ecrite en
// dur — sinon elle derive a chaque release.
void getVersion()
  .then((v) => {
    $("app-version").textContent = `v${v}`;
    $("footer-version").textContent = `avash v${v}`;
  })
  .catch(() => {
    $("app-version").textContent = "v?";
  });

// ---------- Connexion directe (sans ~/.ssh/config) ----------

type ManualTarget = {
  addr: string;
  port: number | null;
  user: string;
  password: string | null;
  key_path: string | null;
};

const manualModal = () => $("manual-modal");
const manualError = () => $("m-error");

function manualOpen() {
  manualError().hidden = true;
  manualSyncProto();
  manualModal().classList.add("open");
  ($("m-addr") as HTMLInputElement).focus();
}

/** Adapte le formulaire au protocole : RDP n'a ni clé, ni sauvegarde config. */
function manualSyncProto() {
  const rdp = (document.querySelector('input[name="proto"]:checked') as HTMLInputElement | null)?.value === "rdp";
  $("m-auth-switch").hidden = rdp;
  $("m-key-row").hidden = rdp || (document.querySelector('input[name="auth"]:checked') as HTMLInputElement | null)?.value !== "key";
  $("m-save-row").hidden = rdp;
  $("m-alias-row").hidden = rdp || !($("m-save") as HTMLInputElement).checked;
  // Mot de passe toujours visible en RDP (seule auth), sinon selon le mode.
  if (rdp) $("m-password-row").hidden = false;
  else manualSyncAuthRows();
  $("m-rdp-remember-row").hidden = !rdp;
  $("m-rdp-save-row").hidden = !rdp;
  $("m-rdp-name-row").hidden = !rdp || !($("m-rdp-save") as HTMLInputElement).checked;
  ($("m-port") as HTMLInputElement).placeholder = rdp ? "3389" : "22";
  ($("m-password") as HTMLInputElement).placeholder = "";
}

function manualClose() {
  manualModal().classList.remove("open");
  ($("manual-form") as HTMLFormElement).reset();
  manualSyncAuthRows();
  manualSyncSaveRow();
  manualSyncProto();
}

/** N'affiche que le champ correspondant au mode d'authentification choisi. */
function manualSyncSaveRow() {
  $("m-alias-row").hidden = !($("m-save") as HTMLInputElement).checked;
}

function manualSyncAuthRows() {
  const mode = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement | null)?.value;
  $("m-password-row").hidden = mode !== "password";
  $("m-key-row").hidden = mode !== "key";
}

function manualReadForm(): ManualTarget {
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const mode = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement).value;
  const portRaw = val("m-port");
  return {
    addr: val("m-addr"),
    port: portRaw ? Number(portRaw) : null,
    user: val("m-user"),
    password: mode === "password" ? val("m-password") || null : null,
    key_path: mode === "key" ? val("m-key") || null : null,
  };
}

async function manualSubmit(ev: Event) {
  ev.preventDefault();
  const submit = $("m-submit") as HTMLButtonElement;
  const proto = (document.querySelector('input[name="proto"]:checked') as HTMLInputElement | null)?.value ?? "ssh";
  if (proto === "rdp") {
    // Bureau distant : on passe par le sidecar, pas de sauvegarde ~/.ssh.
    const addr = ($("m-addr") as HTMLInputElement).value.trim();
    const user = ($("m-user") as HTMLInputElement).value.trim();
    const password = ($("m-password") as HTMLInputElement).value;
    if (!addr || !user) { manualError().textContent = "Adresse et utilisateur requis."; manualError().hidden = false; return; }
    const portRaw = ($("m-port") as HTMLInputElement).value.trim();
    const rport2 = portRaw ? Number(portRaw) : 3389;
    const rport = rport2;
    const enregistrer = ($("m-rdp-save") as HTMLInputElement).checked;
    const memoriser = ($("m-rdp-remember") as HTMLInputElement).checked;
    const nomRdp = ($("m-rdp-name") as HTMLInputElement).value.trim();
    // Le volet SSH refuse un alias vide avant d'enregistrer ; le volet RDP
    // acceptait n'importe quoi et posait dans la barre latérale une ligne sans
    // libellé, que même sa suppression ne savait plus nommer.
    if (enregistrer && !nomRdp) {
      manualError().textContent = "Donne un nom à ce bureau pour l'enregistrer.";
      manualError().hidden = false;
      return;
    }
    submit.disabled = true;
    const libelleRdp = submit.textContent;
    submit.textContent = "Connexion…";
    try {
      if (enregistrer) {
        await invoke("rdp_host_save", {
          id: null, name: nomRdp,
          host: addr, port: rport, user, width: 0, height: 0,
        });
      }
      // « Mémoriser le mot de passe » était imbriqué dans « Enregistrer la
      // connexion » : cochée seule, la case ne faisait rien et le mot de passe
      // était redemandé à la connexion suivante, sans le moindre message.
      if (memoriser && password) {
        await invoke("rdp_password_save", { host: addr, port: rport, user, password });
      }
      if (enregistrer || memoriser) await loadHosts();
    } catch (e) {
      manualError().textContent = e instanceof Error ? e.message : String(e);
      manualError().hidden = false;
      return;
    } finally {
      submit.disabled = false;
      submit.textContent = libelleRdp;
    }
    manualClose();
    await openRdp({ host: addr, port: rport, user, password });
    return;
  }
  const target = manualReadForm();
  manualError().hidden = true;
  submit.disabled = true;
  submit.textContent = "Connexion…";
  try {
    if (($("m-save") as HTMLInputElement).checked) {
      // Enregistrer AVANT de connecter : si l'ecriture echoue (alias deja
      // pris, fichier illisible), l'utilisateur le voit dans le formulaire
      // plutot que de decouvrir plus tard que rien n'a ete sauve.
      const alias = ($("m-alias") as HTMLInputElement).value.trim();
      if (!alias) throw new Error("Donne un nom à l'hôte pour l'enregistrer.");
      await invoke("host_save", {
        alias,
        addr: target.addr,
        port: target.port,
        user: target.user,
        keyPath: target.key_path,
        proxyJump: null,
        tags: null,
      });
      await loadHosts();
    }
    await openManualSession(target);
    manualClose();
  } catch (e) {
    // Le backend renvoie un message deja redige pour l'utilisateur
    // (cle introuvable, cle d'hote modifiee, identifiants manquants).
    manualError().textContent = e instanceof Error ? e.message : String(e);
    manualError().hidden = false;
  } finally {
    submit.disabled = false;
    submit.textContent = "Se connecter";
  }
}

// Cablage du formulaire de connexion directe.
$("manual-btn").addEventListener("click", manualOpen);
$("m-cancel").addEventListener("click", manualClose);
$("manual-form").addEventListener("submit", manualSubmit);
$("m-save").addEventListener("change", manualSyncSaveRow);
// Pre-remplir le nom avec l'adresse : c'est presque toujours ce qu'on veut.
$("m-addr").addEventListener("blur", () => {
  const alias = $("m-alias") as HTMLInputElement;
  if (!alias.value.trim()) alias.value = ($("m-addr") as HTMLInputElement).value.trim();
});
document
  .querySelectorAll('input[name="auth"]')
  .forEach((r) => r.addEventListener("change", manualSyncAuthRows));
document.querySelectorAll('input[name="proto"]').forEach((r) => r.addEventListener("change", manualSyncProto));
$("m-rdp-save").addEventListener("change", manualSyncProto);
// Fermeture à Échap seulement (pas au clic dehors : évite de perdre la saisie).
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && manualModal().classList.contains("open")) manualClose();
});
manualSyncAuthRows();


// ---------- Clés SSH : lister, générer, déployer ----------

type KeyEntry = {
  name: string;
  path: string;
  public_line: string | null;
  mode: string;
};

const keysModal = () => $("keys-modal");
const keyError = () => $("k-error");
const keyOk = () => $("k-ok");

function keyFeedback(msg: string, kind: "ok" | "error") {
  const el = kind === "ok" ? keyOk() : keyError();
  const other = kind === "ok" ? keyError() : keyOk();
  el.textContent = msg;
  el.hidden = false;
  other.hidden = true;
}

function keyFeedbackClear() {
  keyOk().hidden = true;
  keyError().hidden = true;
}

async function keysRefresh() {
  const list = $("key-list");
  const select = $("d-key") as HTMLSelectElement;
  list.innerHTML = "";
  select.innerHTML = "";
  let keys: KeyEntry[];
  try {
    keys = await invoke<KeyEntry[]>("keys_list");
  } catch (e) {
    keyFeedback(String(e), "error");
    return;
  }
  if (keys.length === 0) {
    const empty = document.createElement("div");
    empty.className = "key-empty";
    empty.textContent = "Aucune clé pour l'instant — crée-en une ci-dessous.";
    list.appendChild(empty);
  }
  for (const k of keys) {
    const row = document.createElement("div");
    row.className = "key-row";

    const name = document.createElement("span");
    name.className = "kname";
    name.textContent = k.name;

    // Des droits trop ouverts font refuser la cle par OpenSSH : on le signale
    // plutot que de laisser l'utilisateur devant un echec incomprehensible.
    const mode = document.createElement("span");
    mode.className = "kmode" + (k.mode === "600" ? "" : " warn");
    mode.textContent = k.mode === "600" ? "600" : `${k.mode} ⚠`;
    mode.title = k.mode === "600" ? "Droits corrects" : "OpenSSH exige 600 sur une clé privée";

    row.append(name, mode);

    if (k.public_line) {
      const copy = document.createElement("button");
      copy.className = "kcopy";
      copy.type = "button";
      copy.textContent = "copier la publique";
      copy.addEventListener("click", async () => {
        await navigator.clipboard.writeText(k.public_line!);
        copy.textContent = "copiée ✓";
        setTimeout(() => (copy.textContent = "copier la publique"), 1500);
      });
      row.appendChild(copy);

      const opt = document.createElement("option");
      opt.value = k.public_line;
      opt.textContent = k.name;
      select.appendChild(opt);
    }
    list.appendChild(row);
  }
  // Sans clé, le déploiement n'a pas d'objet.
  ($("deploy-block") as HTMLDetailsElement).hidden = keys.length === 0;
}

async function keysOpen() {
  keyFeedbackClear();
  keysModal().classList.add("open");
  await keysRefresh();
}

function keysClose() {
  keysModal().classList.remove("open");
}

async function keygenSubmit(ev: Event) {
  ev.preventDefault();
  const btn = $("k-gen-submit") as HTMLButtonElement;
  const name = ($("k-name") as HTMLInputElement).value.trim();
  const comment = ($("k-comment") as HTMLInputElement).value.trim();
  btn.disabled = true;
  try {
    const k = await invoke<KeyEntry>("key_generate", { name, comment: comment || null });
    keyFeedback(`Clé « ${k.name} » créée dans ${k.path}`, "ok");
    ($("keygen-form") as HTMLFormElement).reset();
    await keysRefresh();
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
  }
}

async function deploySubmit(ev: Event) {
  ev.preventDefault();
  const btn = $("d-submit") as HTMLButtonElement;
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const portRaw = val("d-port");
  btn.disabled = true;
  btn.textContent = "Installation…";
  try {
    const msg = await invoke<string>("key_deploy", {
      addr: val("d-addr"),
      port: portRaw ? Number(portRaw) : null,
      user: val("d-user"),
      password: val("d-password"),
      publicLine: ($("d-key") as HTMLSelectElement).value,
    });
    keyFeedback(msg, "ok");
    // Le mot de passe ne doit pas trainer dans le formulaire une fois servi.
    ($("d-password") as HTMLInputElement).value = "";
  } catch (e) {
    keyFeedback(String(e), "error");
  } finally {
    btn.disabled = false;
    btn.textContent = "Installer la clé";
  }
}

$("keys-btn").addEventListener("click", keysOpen);
$("k-close").addEventListener("click", keysClose);
$("keygen-form").addEventListener("submit", keygenSubmit);
$("deploy-form").addEventListener("submit", deploySubmit);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && keysModal().classList.contains("open")) keysClose();
});

// ---------- Raccourcis d'onglets ----------

/** Liste ordonnee des identifiants de session, pour naviguer par position. */
/** Tous les onglets (SSH + RDP) dans l'ordre du DOM. */
function orderedTabs(): { kind: "ssh" | "rdp"; id: number }[] {
  const byEl = new Map<HTMLElement, { kind: "ssh" | "rdp"; id: number }>();
  for (const [id, sess] of state.sessions) byEl.set(sess.tab, { kind: "ssh", id });
  for (const [id, sess] of rdpSessions) byEl.set(sess.tab, { kind: "rdp", id });
  const out: { kind: "ssh" | "rdp"; id: number }[] = [];
  for (const el of $("tabs").querySelectorAll<HTMLElement>(".tab")) {
    const t = byEl.get(el);
    if (t) out.push(t);
  }
  return out;
}
function focusTab(t: { kind: "ssh" | "rdp"; id: number }) {
  if (t.kind === "ssh") focusSession(t.id);
  else focusRdp(t.id);
}
function closeActiveTab() {
  if (state.active === null) return;
  if (rdpSessions.has(state.active)) closeRdp(state.active);
  else closeSession(state.active);
}

window.addEventListener("keydown", (e) => {
  // Ne pas capturer pendant qu'un formulaire OU la palette est ouvert :
  // l'utilisateur y tape, Ctrl+W fermerait un onglet sous ses doigts.
  if (document.querySelector(".modal-backdrop.open, .palette-backdrop.open")) return;
  const mod = e.ctrlKey || e.metaKey;
  if (!mod) return;

  if (e.key.toLowerCase() === "w" && state.active !== null) {
    e.preventDefault();
    closeActiveTab();
    return;
  }
  const tabs = orderedTabs();
  if (e.key === "Tab") {
    if (tabs.length < 2) return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === state.active);
    const step = e.shiftKey ? -1 : 1;
    focusTab(tabs[(Math.max(0, i) + step + tabs.length) % tabs.length]);
    return;
  }
  // Ctrl+1..9 : acces direct a un onglet (SSH ou RDP) par sa position.
  if (/^[1-9]$/.test(e.key)) {
    const idx = Number(e.key) - 1;
    if (idx < tabs.length) {
      e.preventDefault();
      focusTab(tabs[idx]);
    }
  }
});

// L'ecran d'accueil s'adapte : sans hote declare, le conseil « double-clic
// sur un hote » est inapplicable.
$("empty-connect").addEventListener("click", manualOpen);
$("empty-keys").addEventListener("click", keysOpen);
manualSyncSaveRow();


// ---------- Demande de mot de passe ----------

const passModal = () => $("pass-modal");
let passResolve: ((v: { password: string; remember: boolean } | null) => void) | null = null;

/**
 * Demande un mot de passe et rend la reponse.
 *
 * Rend `null` si l'utilisateur annule — l'appelant doit alors renoncer
 * proprement plutot que de tenter une connexion sans identifiant.
 */
function askPassword(target: string, erreur?: string): Promise<{ password: string; remember: boolean } | null> {
  $("pass-target").textContent = target;
  ($("pass-input") as HTMLInputElement).value = "";
  ($("pass-remember") as HTMLInputElement).checked = false;
  const err = $("pass-error");
  if (erreur) {
    err.textContent = erreur;
    err.hidden = false;
  } else {
    err.hidden = true;
  }
  passModal().classList.add("open");
  setTimeout(() => ($("pass-input") as HTMLInputElement).focus(), 30);
  return new Promise((resolve) => {
    passResolve?.(null); // cf. askText : ne jamais abandonner une promesse
    passResolve = resolve;
  });
}

function passClose(value: { password: string; remember: boolean } | null) {
  passModal().classList.remove("open");
  const r = passResolve;
  passResolve = null;
  r?.(value);
}

$("pass-form").addEventListener("submit", (e) => {
  e.preventDefault();
  passClose({
    password: ($("pass-input") as HTMLInputElement).value,
    remember: ($("pass-remember") as HTMLInputElement).checked,
  });
});
$("pass-cancel").addEventListener("click", () => passClose(null));
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && passModal().classList.contains("open")) {
    e.stopImmediatePropagation(); // voir la note du gestionnaire de confirmation
    passClose(null);
  }
});


// ---------- Zoom de police, recherche, menu clic droit ----------

/** Applique une taille de police à tous les terminaux et la borne. */
function setFontSize(px: number) {
  terminalFontSize = Math.max(FONT_MIN, Math.min(FONT_MAX, px));
  for (const s of state.sessions.values()) {
    s.term.options.fontSize = terminalFontSize;
    s.fit.fit();
    invoke("pty_resize", { id: s.id, cols: s.term.cols, rows: s.term.rows }).catch(() => {});
  }
}

/** Ouvre la barre de recherche du terminal actif. */
function openTermSearch(id: number) {
  if (state.active !== id) return;
  const bar = $("term-search");
  bar.classList.add("open");
  const input = $("term-search-input") as HTMLInputElement;
  input.value = "";
  input.focus();
}

function closeTermSearch() {
  $("term-search").classList.remove("open");
  const s = state.active !== null ? state.sessions.get(state.active) : undefined;
  s?.search.clearDecorations();
  s?.term.focus();
}

function runTermSearch(next: boolean) {
  if (state.active === null) return;
  const s = state.sessions.get(state.active);
  if (!s) return;
  const q = ($("term-search-input") as HTMLInputElement).value;
  if (!q) return;
  const opts = { decorations: { matchOverviewRuler: "#8b7cf6", activeMatchColorOverviewRuler: "#a598f8" } };
  if (next) s.search.findNext(q, opts);
  else s.search.findPrevious(q, opts);
}

$("term-search-input").addEventListener("keydown", (e) => {
  const k = e as KeyboardEvent;
  if (k.key === "Enter") { k.preventDefault(); runTermSearch(!k.shiftKey); }
  else if (k.key === "Escape") { k.preventDefault(); closeTermSearch(); }
});
$("term-search-next").addEventListener("click", () => runTermSearch(true));
$("term-search-prev").addEventListener("click", () => runTermSearch(false));
$("term-search-close").addEventListener("click", closeTermSearch);

// Menu contextuel du terminal : copier / coller / rechercher / tout sélectionner.
const ctxMenu = () => $("term-context");
function hideContext() { ctxMenu().classList.remove("open"); }

$("terminal").addEventListener("contextmenu", (e) => {
  e.preventDefault();
  if (state.active === null) return;
  const m = ctxMenu();
  const s = state.sessions.get(state.active);
  const hasSel = !!s?.term.getSelection();
  // Griser « copier » sans sélection.
  (m.querySelector('[data-act="copy"]') as HTMLElement).classList.toggle("disabled", !hasSel);
  m.style.left = `${(e as MouseEvent).clientX}px`;
  m.style.top = `${(e as MouseEvent).clientY}px`;
  m.classList.add("open");
});
window.addEventListener("click", hideContext);
window.addEventListener("blur", hideContext);
ctxMenu().addEventListener("click", (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const s = state.active !== null ? state.sessions.get(state.active) : undefined;
  if (!s) return;
  if (act === "copy") {
    const sel = s.term.getSelection();
    if (sel) navigator.clipboard.writeText(sel).catch(() => {});
  } else if (act === "paste") {
    navigator.clipboard.readText().then(
      (t) => invoke("pty_write", { id: s.id, data: t }).catch(() => {}),
      () => {},
    );
  } else if (act === "search") {
    openTermSearch(s.id);
  } else if (act === "selectall") {
    s.term.selectAll();
  } else if (act === "clear") {
    s.term.clear();
  }
  hideContext();
});


// ---------- Menu contextuel d'un hôte ----------

const MENUS_CONTEXTUELS = ["host-context", "rdp-context", "folder-context", "sftp-context"];

function closeAllContextMenus() {
  for (const id of MENUS_CONTEXTUELS) $(id).classList.remove("open");
}

/** Rend un menu contextuel utilisable au clavier, après Maj+F10.
 *
 *  Le menu s'ouvrait mais le focus restait sur la ligne : les flèches
 *  continuaient de déplacer la sélection *derrière* le menu resté ouvert,
 *  Entrée relançait la connexion par-dessus, et Échap ne fermait rien — seul un
 *  clic de souris en sortait, ce qui annulait la raison d'être du raccourci.
 *
 *  Le focus revient à l'élément d'où l'on vient, comme pour les modales. */
function ouvrirMenuAuClavier(menu: HTMLElement, origine: HTMLElement): void {
  const items = [...menu.querySelectorAll<HTMLElement>("[data-act]")].filter((i) => !i.hidden);
  if (items.length === 0) return;
  for (const i of items) i.tabIndex = -1;
  items[0].focus();
  const surTouche = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      fermer();
      return;
    }
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const pas = e.key === "ArrowDown" ? 1 : -1;
      items[(i + pas + items.length) % items.length].focus();
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const choisi = items[i] ?? items[0];
      fermer();
      choisi.click();
    } else if (e.key === "Tab") {
      // Une tabulation sort du menu : on le referme plutôt que de laisser un
      // menu ouvert sans focus dedans.
      fermer();
    }
  };
  function fermer() {
    menu.removeEventListener("keydown", surTouche, true);
    menu.classList.remove("open");
    if (origine.isConnected) origine.focus();
  }
  menu.addEventListener("keydown", surTouche, true);
}
/**
 * Positionne un menu contextuel en le gardant dans la fenêtre.
 *
 * Le menu des hôtes SSH — le plus haut des cinq, sept entrées — posait
 * brutalement les coordonnées du clic : un clic droit sur le dernier hôte d'une
 * liste descendant jusqu'en bas rendait « Supprimer l'hôte » inatteignable.
 * On mesure la taille réelle plutôt que de la supposer.
 */
function placerMenu(menu: HTMLElement, e: MouseEvent): void {
  menu.style.visibility = "hidden";
  menu.classList.add("open");
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(e.clientX, window.innerWidth - width - 8))}px`;
  menu.style.top = `${Math.max(4, Math.min(e.clientY, window.innerHeight - height - 8))}px`;
  menu.style.visibility = "";
}

function openHostMenu(h: Host, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("host-context");
  m.dataset.alias = h.alias;
  placerMenu(m, e);
  m.classList.add("open");
}
function hideHostMenu() { $("host-context").classList.remove("open"); }
window.addEventListener("click", hideHostMenu);
window.addEventListener("blur", hideHostMenu);

$("host-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const alias = $("host-context").dataset.alias;
  hideHostMenu();
  if (!alias) return;
  const h = state.hosts.find((x) => x.alias === alias);
  if (!h) return;
  if (act === "connect") {
    void openSession(h);
  } else if (act === "edit") {
    await openEditHost(alias);
  } else if (act === "move") {
    openMoveModal("ssh", alias);
  } else if (act === "tunnels") {
    await tunnelsOpen(alias);
  } else if (act === "delete") {
    const ok = await askConfirm(
      `Supprimer l'hôte « ${alias} » de ~/.ssh/config ?\n\n` +
        `Son mot de passe mémorisé sera aussi oublié. Cette action est définitive.`,
    );
    if (!ok) return;
    try {
      await invoke("host_delete", { alias });
      await loadHosts();
    } catch (err) {
      notifyErreur(`Suppression impossible : ${err}`);
    }
  } else if (act === "forget") {
    // L'action ne disait rien du tout : ni succès, ni échec. On ne pouvait pas
    // savoir si le trousseau avait été purgé ou si le clic avait manqué sa
    // cible — et un échec réel (trousseau verrouillé, D-Bus absent) laissait
    // croire le secret effacé alors qu'il était toujours là.
    try {
      await invoke("password_forget", {
        addr: h.hostname ?? h.alias,
        port: h.port,
        user: h.user ?? null,
      });
      notify(`Mot de passe oublié pour ${h.alias}.`, "succes");
    } catch (err) {
      notifyErreur(`Le mot de passe n'a pas pu être oublié : ${err}`);
    }
  }
});


// ---------- Modifier un hôte ----------

async function openEditHost(alias: string) {
  const err = $("e-error");
  err.hidden = true;
  try {
    const h = await invoke<Host>("host_get", { alias });
    ($("e-old") as HTMLInputElement).value = h.alias;
    ($("e-alias") as HTMLInputElement).value = h.alias;
    ($("e-addr") as HTMLInputElement).value = h.hostname ?? "";
    ($("e-port") as HTMLInputElement).value = h.port ? String(h.port) : "";
    ($("e-user") as HTMLInputElement).value = h.user ?? "";
    ($("e-key") as HTMLInputElement).value = h.identity_file ?? "";
    ($("e-jump") as HTMLInputElement).value = h.proxy_jump ?? "";
    ($("e-tags") as HTMLInputElement).value = h.tags.join(", ");
    ($("edit-form") as HTMLFormElement).dataset.folder = h.folder ?? "";
    $("edit-modal").classList.add("open");
    setTimeout(() => ($("e-alias") as HTMLInputElement).focus(), 30);
  } catch (e) {
    notifyErreur(`Impossible de charger l'hôte : ${e}`);
  }
}

function closeEditHost() { $("edit-modal").classList.remove("open"); }

$("e-cancel").addEventListener("click", closeEditHost);
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // Une boîte ouverte PAR-DESSUS (confirmation, saisie, mot de passe) a déjà
  // traité la touche : sans cette garde, renoncer à une suppression fermait
  // aussi la fenêtre Tunnels ou Snippets d'où l'on venait — l'utilisateur qui
  // annule était puni deux fois.
  if (MODALES_AU_DESSUS.some((id) => $(id).classList.contains("open"))) return;
  // Un seul `return` par branche : elles s'enchaînaient toutes.
  if ($("edit-modal").classList.contains("open")) { closeEditHost(); return; }
  if ($("rdp-edit-modal").classList.contains("open")) { closeEditRdp(); return; }
  if ($("move-modal").classList.contains("open")) { closeMoveModal(); return; }
  if ($("tunnels-modal").classList.contains("open")) { tunnelsClose(); return; }
  if (snippetsModal().classList.contains("open")) { snippetsClose(); return; }
});

$("edit-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const portRaw = val("e-port");
  const err = $("e-error");
  const submit = $("e-submit") as HTMLButtonElement;
  submit.disabled = true;
  try {
    await invoke("host_update", {
      oldAlias: val("e-old"),
      alias: val("e-alias"),
      addr: val("e-addr"),
      port: portRaw ? Number(portRaw) : null,
      user: val("e-user") || null,
      keyPath: val("e-key") || null,
      proxyJump: val("e-jump") || null,
      tags: val("e-tags") || null,
      folder: ($("edit-form") as HTMLFormElement).dataset.folder ?? null,
    });
    closeEditHost();
    await loadHosts();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});


// ---------- Tunnels SSH ----------

const tunnels = {
  defs: [] as TunnelDef[],
  status: new Map<string, TunnelStatus>(),
  /** Alias -> nombre de tunnels vivants (badge de la barre laterale). */
  byHost: new Map<string, number>(),
  /** Alias preselectionne quand la modale s'ouvre depuis un hote. */
  focusAlias: null as string | null,
  timer: null as number | null,
  /** Tunnels en cours de demarrage : evite le double clic. */
  busy: new Set<string>(),
};

const tunnelsModal = () => $("tunnels-modal");

/** Recharge definitions + etats, puis redessine liste et badges. */
async function tunnelsRefresh() {
  try {
    const [defs, status] = await Promise.all([
      invoke<TunnelDef[]>("tunnel_defs"),
      invoke<TunnelStatus[]>("tunnel_status"),
    ]);
    tunnels.defs = defs;
    tunnels.status = new Map(status.map((s) => [s.id, s]));
  } catch (e) {
    $("t-error").textContent = String(e);
    $("t-error").hidden = false;
    return;
  }
  const before = tunnels.byHost;
  tunnels.byHost = activeTunnelsByHost(tunnels.defs, tunnels.status);
  // Redessiner la liste d'hotes a chaque tick coûterait pour rien : on ne le
  // fait que si un badge change.
  const changed =
    before.size !== tunnels.byHost.size ||
    [...tunnels.byHost].some(([k, v]) => before.get(k) !== v);
  if (changed) renderHosts();
  if (tunnelsModal().classList.contains("open")) renderTunnels();
}

function renderTunnels() {
  const list = $("tunnel-list");
  list.innerHTML = "";
  // L'hote d'origine en tete, le reste ensuite : on voit d'abord ce pour
  // quoi on a ouvert la modale, sans perdre la vue d'ensemble.
  const defs = [...tunnels.defs].sort((a, b) => {
    const fa = a.alias === tunnels.focusAlias ? 0 : 1;
    const fb = b.alias === tunnels.focusAlias ? 0 : 1;
    return fa - fb || a.alias.localeCompare(b.alias) || a.bind_port - b.bind_port;
  });
  if (defs.length === 0) {
    const empty = document.createElement("div");
    empty.className = "tunnel-empty";
    empty.textContent = "Aucun tunnel défini. Crée-en un ci-dessous.";
    list.appendChild(empty);
    return;
  }
  for (const d of defs) {
    const st = tunnels.status.get(d.id);
    const running = !!st;
    const alive = !!st?.alive;
    const row = document.createElement("div");
    row.className = "tunnel-row" + (running ? (alive ? " alive" : " dead") : "");
    row.innerHTML = `<span class="tdot"></span>
      <div class="tmain">
        <div class="ttitle"><span class="tflag"></span><span class="tname"></span></div>
        <div class="tdesc"></div>
        <div class="tstats"></div>
        <div class="terr" hidden></div>
      </div>
      <div class="tacts">
        <button class="tbtn" data-act="toggle"></button>
        <button class="tbtn" data-act="edit" title="Modifier">${ic("pencil")}</button>
        <button class="tbtn danger" data-act="delete" title="Supprimer">${ic("trash")}</button>
      </div>`;
    row.querySelector(".tflag")!.textContent = tunnelFlag(d.kind);
    row.querySelector(".tname")!.textContent = d.name || d.alias;
    const desc = row.querySelector(".tdesc") as HTMLElement;
    desc.textContent = describeTunnel(d);
    desc.title = describeTunnel(d); // la ligne est tronquee si longue
    const stats = row.querySelector(".tstats")!;
    if (st && alive) stats.textContent = tunnelTraffic(st);
    else if (st) stats.textContent = "Connexion perdue";
    else stats.textContent = "Arrêté";
    const err = row.querySelector(".terr") as HTMLElement;
    if (st?.last_error) {
      err.textContent = `⚠️ ${st.last_error}`;
      err.hidden = false;
    }
    const toggle = row.querySelector('[data-act="toggle"]') as HTMLButtonElement;
    if (tunnels.busy.has(d.id)) {
      toggle.textContent = "…";
      toggle.disabled = true;
    } else if (alive) {
      toggle.innerHTML = `${ic("stop")}<span>Arrêter</span>`;
      toggle.className = "tbtn stop labeled";
    } else {
      toggle.innerHTML = `${ic("refresh")}<span>${running ? "Relancer" : "Démarrer"}</span>`;
      toggle.className = "tbtn go labeled";
    }
    toggle.addEventListener("click", () => tunnelToggle(d));
    row.querySelector('[data-act="edit"]')!.addEventListener("click", () => tunnelEdit(d));
    row.querySelector('[data-act="delete"]')!.addEventListener("click", () => tunnelDelete(d));
    list.appendChild(row);
  }
}

async function tunnelToggle(d: TunnelDef) {
  const st = tunnels.status.get(d.id);
  if (st?.alive) {
    try {
      await invoke("tunnel_stop", { id: d.id });
    } catch (e) {
      notifyErreur(`Arrêt impossible : ${e}`);
    }
    await tunnelsRefresh();
    return;
  }
  await tunnelStart(d);
}

/**
 * Demarre un tunnel, avec le meme dialogue de mot de passe qu'un onglet :
 * demande avant si l'hote n'a rien pour s'authentifier, redemande sur refus.
 */
async function tunnelStart(d: TunnelDef) {
  const h = state.hosts.find((x) => x.alias === d.alias);
  const label = h ? `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}` : d.alias;
  let password: string | null = null;
  let rememberAsked = false;
  try {
    if (await invoke<boolean>("host_needs_password", { alias: d.alias })) {
      const rep = await askPassword(label);
      if (!rep) return;
      password = rep.password;
      rememberAsked = rep.remember;
    }
  } catch {
    /* le backend dira ce qui manque */
  }
  tunnels.busy.add(d.id);
  renderTunnels();
  try {
    for (let essai = 0; essai < 3; essai++) {
      try {
        await invoke("tunnel_start", { id: d.id, password });
        if (password && rememberAsked && h) {
          await invoke("password_save", {
            addr: h.hostname ?? h.alias,
            port: h.port,
            user: h.user ?? null,
            password,
          }).catch(() => { /* facultatif */ });
        }
        return;
      } catch (e) {
        const msg = String(e);
        if (!isPasswordRequired(msg)) {
          $("t-error").textContent = `Démarrage impossible : ${msg}`;
          $("t-error").hidden = false;
          return;
        }
        const rep = await askPassword(label, essai === 0 ? undefined : "Mot de passe refusé, nouvelle tentative.");
        if (!rep) return;
        password = rep.password;
        rememberAsked = rep.remember;
      }
    }
  } finally {
    tunnels.busy.delete(d.id);
    await tunnelsRefresh();
  }
}

async function tunnelDelete(d: TunnelDef) {
  const ok = await askConfirm(`Supprimer le tunnel « ${d.name || describeTunnel(d)} » ?` +
    (tunnels.status.get(d.id)?.alive ? "\n\nIl est actif : il sera coupé." : ""));
  if (!ok) return;
  try {
    await invoke("tunnel_def_delete", { id: d.id });
  } catch (e) {
    notifyErreur(`Suppression impossible : ${e}`);
  }
  await tunnelsRefresh();
}

// ----- Formulaire -----

const KIND_HINTS: Record<TunnelKind, { hint: string; bind: string; host: string }> = {
  local: {
    hint: "Ce que tu ouvres sur ta machine arrive, via le serveur, à la destination (ex. une base de données interne).",
    bind: "Port local d'écoute",
    host: "Destination (vue du serveur)",
  },
  remote: {
    hint: "Ce qui frappe ce port sur le serveur arrive, via ta machine, à la destination (ex. exposer un service local).",
    bind: "Port d'écoute sur le serveur",
    host: "Destination (vue de ta machine)",
  },
  dynamic: {
    hint: "Mandataire SOCKS5 sur ta machine : configure ton navigateur dessus et tout sort par le serveur.",
    bind: "Port local du mandataire",
    host: "",
  },
};

function tunnelKind(): TunnelKind {
  const checked = document.querySelector<HTMLInputElement>('input[name="tkind"]:checked');
  return (checked?.value as TunnelKind) ?? "local";
}

function tunnelSyncKind() {
  const k = KIND_HINTS[tunnelKind()];
  $("t-kind-hint").textContent = k.hint;
  $("t-bind-label").textContent = k.bind;
  $("t-host-label").textContent = k.host;
  $("t-target-row").hidden = tunnelKind() === "dynamic";
  ($("t-bind") as HTMLInputElement).placeholder = tunnelKind() === "dynamic" ? "1080" : "8080";
}

function tunnelFormReset() {
  ($("tunnel-form") as HTMLFormElement).reset();
  ($("t-id") as HTMLInputElement).value = "";
  $("tunnel-form-title").textContent = "Nouveau tunnel";
  $("t-submit").textContent = "Enregistrer";
  $("t-reset").hidden = true;
  $("t-error").hidden = true;
  if (tunnels.focusAlias) ($("t-alias") as HTMLSelectElement).value = tunnels.focusAlias;
  tunnelSyncKind();
}

function tunnelEdit(d: TunnelDef) {
  ($("t-id") as HTMLInputElement).value = d.id;
  ($("t-alias") as HTMLSelectElement).value = d.alias;
  (document.querySelector(`input[name="tkind"][value="${d.kind}"]`) as HTMLInputElement).checked = true;
  ($("t-bind") as HTMLInputElement).value = String(d.bind_port);
  ($("t-host") as HTMLInputElement).value = d.target_host;
  ($("t-port") as HTMLInputElement).value = d.target_port ? String(d.target_port) : "";
  ($("t-name") as HTMLInputElement).value = d.name;
  $("tunnel-form-title").textContent = `Modifier « ${d.name || describeTunnel(d)} »`;
  $("t-submit").textContent = "Enregistrer les modifications";
  $("t-reset").hidden = false;
  ($("tunnel-block") as HTMLDetailsElement).open = true;
  tunnelSyncKind();
  ($("t-bind") as HTMLInputElement).focus();
}

function tunnelFillHosts() {
  const sel = $("t-alias") as HTMLSelectElement;
  const current = sel.value;
  sel.innerHTML = "";
  for (const h of state.hosts) {
    const o = document.createElement("option");
    o.value = h.alias;
    o.textContent = h.alias;
    sel.appendChild(o);
  }
  if (current) sel.value = current;
}

async function tunnelsOpen(alias?: string) {
  tunnels.focusAlias = alias ?? null;
  tunnelFillHosts();
  tunnelFormReset();
  tunnelsModal().classList.add("open");
  await tunnelsRefresh();
  renderTunnels();
  // Sans tunnel encore defini, le formulaire est la seule chose a faire :
  // on l'ouvre d'office.
  ($("tunnel-block") as HTMLDetailsElement).open = tunnels.defs.length === 0;
  if (tunnels.timer !== null) clearInterval(tunnels.timer);
  tunnels.timer = window.setInterval(tunnelsRefresh, 1500);
}

function tunnelsClose() {
  tunnelsModal().classList.remove("open");
  if (tunnels.timer !== null) {
    clearInterval(tunnels.timer);
    tunnels.timer = null;
  }
}

$("tunnels-btn").addEventListener("click", () => tunnelsOpen());
$("t-close").addEventListener("click", tunnelsClose);
$("t-reset").addEventListener("click", tunnelFormReset);
for (const r of document.querySelectorAll('input[name="tkind"]')) {
  r.addEventListener("change", tunnelSyncKind);
}

$("tunnel-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const err = $("t-error");
  const submit = $("t-submit") as HTMLButtonElement;
  const kind = tunnelKind();
  submit.disabled = true;
  try {
    await invoke("tunnel_def_save", {
      id: val("t-id") || null,
      alias: ($("t-alias") as HTMLSelectElement).value,
      kind,
      bindPort: Number(val("t-bind")),
      targetHost: kind === "dynamic" ? null : val("t-host") || null,
      targetPort: kind === "dynamic" || !val("t-port") ? null : Number(val("t-port")),
      name: val("t-name") || null,
    });
    tunnelFormReset();
    ($("tunnel-block") as HTMLDetailsElement).open = false;
    await tunnelsRefresh();
    renderTunnels();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});

// Badges de la barre laterale : un rafraichissement initial, puis toutes les
// 5 s UNIQUEMENT s'il existe des tunnels a surveiller (sinon c'est un
// aller-retour IPC gaspille en continu au repos).
void tunnelsRefresh();
window.setInterval(() => {
  if (tunnels.defs.length === 0) return;
  if (!tunnelsModal().classList.contains("open")) void tunnelsRefresh();
}, 5000);

// ---------- Snippets ----------

type OpenSession = { id: number; label: string };

const snip = {
  list: [] as Snippet[],
  focusId: null as string | null,
};
const snippetsModal = () => $("snippets-modal");

async function snippetsRefresh() {
  try {
    snip.list = await invoke<Snippet[]>("snippet_list");
  } catch (e) {
    $("sn-error").textContent = String(e);
    $("sn-error").hidden = false;
    return;
  }
  renderSnippets();
}

function renderSnippets() {
  const list = $("snippet-list");
  list.innerHTML = "";
  if (snip.list.length === 0) {
    const empty = document.createElement("div");
    empty.className = "snippet-empty";
    empty.textContent = "Aucun snippet. Crée-en un ci-dessous.";
    list.appendChild(empty);
    return;
  }
  for (const sn of [...snip.list].sort((a, b) => a.name.localeCompare(b.name))) {
    const nVars = snippetVars(sn.command).length;
    const row = document.createElement("div");
    row.className = "snippet-row";
    row.innerHTML = `<div class="smain">
        <div class="sname"><span class="snm"></span></div>
        <div class="scmd"></div>
      </div>
      <div class="sacts">
        <button class="tbtn go" data-act="send" title="Envoyer">${ic("play")}</button>
        <button class="tbtn" data-act="edit" title="Modifier">${ic("pencil")}</button>
        <button class="tbtn danger" data-act="delete" title="Supprimer">${ic("trash")}</button>
      </div>`;
    row.querySelector(".snm")!.textContent = sn.name;
    if (nVars > 0) {
      const b = document.createElement("span");
      b.className = "svar";
      b.textContent = `${nVars} var${nVars > 1 ? "s" : ""}`;
      row.querySelector(".sname")!.appendChild(b);
    }
    row.querySelector(".scmd")!.textContent = snippetPreview(sn.command);
    row.querySelector('[data-act="send"]')!.addEventListener("click", () => snippetSendFlow(sn));
    row.querySelector('[data-act="edit"]')!.addEventListener("click", () => snippetEdit(sn));
    row.querySelector('[data-act="delete"]')!.addEventListener("click", () => snippetDelete(sn));
    list.appendChild(row);
  }
}

// ----- Formulaire -----

function snippetFormReset() {
  ($("snippet-form") as HTMLFormElement).reset();
  ($("sn-id") as HTMLInputElement).value = "";
  ($("sn-run") as HTMLInputElement).checked = true;
  $("snippet-form-title").textContent = "Nouveau snippet";
  $("sn-submit").textContent = "Enregistrer";
  $("sn-reset").hidden = true;
  $("sn-error").hidden = true;
  snippetSyncVars();
}

function snippetSyncVars() {
  const vars = snippetVars(($("sn-command") as HTMLTextAreaElement).value);
  $("sn-vars").textContent = vars.length ? `Variables : ${vars.map((v) => `{{${v}}}`).join(", ")}` : "";
}

function snippetEdit(sn: Snippet) {
  ($("sn-id") as HTMLInputElement).value = sn.id;
  ($("sn-name") as HTMLInputElement).value = sn.name;
  ($("sn-command") as HTMLTextAreaElement).value = sn.command;
  ($("sn-run") as HTMLInputElement).checked = sn.run;
  $("snippet-form-title").textContent = `Modifier « ${sn.name} »`;
  $("sn-submit").textContent = "Enregistrer les modifications";
  $("sn-reset").hidden = false;
  ($("snippet-block") as HTMLDetailsElement).open = true;
  snippetSyncVars();
  ($("sn-name") as HTMLInputElement).focus();
}

async function snippetDelete(sn: Snippet) {
  if (!(await askConfirm(`Supprimer le snippet « ${sn.name} » ?`))) return;
  try {
    await invoke("snippet_delete", { id: sn.id });
    await snippetsRefresh();
  } catch (e) {
    notifyErreur(`Suppression impossible : ${e}`);
  }
}

async function snippetsOpen() {
  snippetFormReset();
  snippetsModal().classList.add("open");
  await snippetsRefresh();
  ($("snippet-block") as HTMLDetailsElement).open = snip.list.length === 0;
}
function snippetsClose() { snippetsModal().classList.remove("open"); }

$("snippets-btn").addEventListener("click", snippetsOpen);
$("sn-close").addEventListener("click", snippetsClose);
$("sn-reset").addEventListener("click", snippetFormReset);
$("sn-command").addEventListener("input", snippetSyncVars);

$("snippet-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const err = $("sn-error");
  const submit = $("sn-submit") as HTMLButtonElement;
  submit.disabled = true;
  try {
    await invoke("snippet_save", {
      id: ($("sn-id") as HTMLInputElement).value || null,
      name: ($("sn-name") as HTMLInputElement).value.trim(),
      command: ($("sn-command") as HTMLTextAreaElement).value,
      run: ($("sn-run") as HTMLInputElement).checked,
      category: null,
    });
    snippetFormReset();
    ($("snippet-block") as HTMLDetailsElement).open = false;
    await snippetsRefresh();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});

// ----- Flux d'envoi : variables puis cibles -----

let sendCtx: { snippet: Snippet; sessions: OpenSession[] } | null = null;

async function snippetSendFlow(sn: Snippet) {
  let sessions: OpenSession[];
  try {
    sessions = await invoke<OpenSession[]>("open_sessions");
  } catch {
    sessions = [];
  }
  if (sessions.length === 0) {
    $("sn-error").textContent = "Ouvre d'abord une session : un snippet s'envoie dans un terminal.";
    $("sn-error").hidden = false;
    return;
  }
  sendCtx = { snippet: sn, sessions };
  $("send-title").textContent = sn.name;
  ($("send-run") as HTMLInputElement).checked = sn.run;

  // Champs pour chaque variable.
  const varsBox = $("send-vars");
  varsBox.innerHTML = "";
  for (const v of snippetVars(sn.command)) {
    // Le nom de variable vient du snippet : on le pose via le DOM (dataset,
    // textContent) et jamais via innerHTML, pour qu'un « " » ou « > » dans un
    // nom ne casse pas l'attribut.
    const label = document.createElement("label");
    const span = document.createElement("span");
    span.textContent = v;
    const input = document.createElement("input");
    input.spellcheck = false;
    input.dataset.var = v;
    label.append(span, input);
    varsBox.appendChild(label);
  }

  // Cibles : cases a cocher si plusieurs sessions ; l'active pre-cochee.
  const wrap = $("send-targets-wrap");
  const targets = $("send-targets");
  targets.innerHTML = "";
  if (sessions.length > 1) {
    wrap.hidden = false;
    for (const se of sessions) {
      const label = document.createElement("label");
      const checked = se.id === state.active ? "checked" : "";
      label.innerHTML = `<input type="checkbox" data-sid="${se.id}" ${checked}/><span></span>`;
      label.querySelector("span")!.textContent = se.label;
      targets.appendChild(label);
    }
  } else {
    wrap.hidden = true;
  }

  updateSendPreview();
  $("send-error").hidden = true;
  $("send-modal").classList.add("open");
  setTimeout(() => (varsBox.querySelector("input") as HTMLInputElement | null)?.focus(), 30);
}

function currentVars(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const i of $("send-vars").querySelectorAll<HTMLInputElement>("input[data-var]")) {
    out[i.dataset.var!] = i.value;
  }
  return out;
}

function updateSendPreview() {
  if (!sendCtx) return;
  $("send-preview").textContent = renderSnippet(sendCtx.snippet.command, currentVars());
}

$("send-vars").addEventListener("input", updateSendPreview);
$("send-cancel").addEventListener("click", () => $("send-modal").classList.remove("open"));
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("send-modal").classList.contains("open")) $("send-modal").classList.remove("open");
});

$("send-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!sendCtx) return;
  const command = renderSnippet(sendCtx.snippet.command, currentVars());
  const run = ($("send-run") as HTMLInputElement).checked;
  let ids: number[];
  if (sendCtx.sessions.length > 1) {
    ids = [...$("send-targets").querySelectorAll<HTMLInputElement>("input:checked")].map((i) => Number(i.dataset.sid));
    if (ids.length === 0) {
      $("send-error").textContent = "Choisis au moins une session.";
      $("send-error").hidden = false;
      return;
    }
  } else {
    ids = [sendCtx.sessions[0].id];
  }
  try {
    const n = await invoke<number>("snippet_send", { sessionIds: ids, command, run });
    $("send-modal").classList.remove("open");
    snippetsClose();
    // Retour a l'onglet vise (le premier), pour voir le resultat.
    focusSession(ids[0]);
    void n;
  } catch (ex) {
    $("send-error").textContent = String(ex);
    $("send-error").hidden = false;
  }
});


// ---------- Barre de titre custom (decorations: false) ----------

async function setupWindowControls() {
  const win = getCurrentWindow();
  $("win-min").innerHTML = ic("winMin");
  $("win-close").innerHTML = ic("winClose");
  const maxBtn = $("win-max");
  // On ne réécrit le bouton que si l'état a réellement changé : réinjecter le
  // même SVG force une réanalyse HTML pour rien.
  let maxAffiche: boolean | null = null;
  const paintMax = async () => {
    const maximisee = await win.isMaximized();
    if (maximisee === maxAffiche) return;
    maxAffiche = maximisee;
    maxBtn.innerHTML = ic(maximisee ? "winRestore" : "winMax");
  };
  await paintMax();
  $("win-min").addEventListener("click", () => win.minimize());
  maxBtn.addEventListener("click", async () => { await win.toggleMaximize(); await paintMax(); });
  $("win-close").addEventListener("click", () => win.close());
  // L'état maximisé change aussi via double-clic système / raccourci.
  //
  // onResized se déclenche à CHAQUE image pendant qu'on tire la fenêtre. Y
  // enchaîner un aller-retour IPC (isMaximized) saturait le pont avec le
  // backend et figeait l'interface au bout de quelques secondes de glissé —
  // sans même qu'une session soit ouverte. L'état maximisé ne peut changer
  // qu'au terme du geste : on attend que le redimensionnement se pose.
  let repeindreMax: number | undefined;
  void win.onResized(() => {
    window.clearTimeout(repeindreMax);
    repeindreMax = window.setTimeout(() => void paintMax(), 150);
  });
  // Fenêtre en arrière-plan : geler les animations (CPU au repos ~0).
  void win.onFocusChanged(({ payload: focused }) => {
    document.body.classList.toggle("win-blur", !focused);
  });

  // Poignées de redimensionnement : sans décorations, la fenêtre n'a plus de
  // bords redimensionnables (surtout sous Wayland). On les recrée nous-mêmes.
  const dirs: [string, string][] = [
    ["rh-n", "North"], ["rh-s", "South"], ["rh-e", "East"], ["rh-w", "West"],
    ["rh-ne", "NorthEast"], ["rh-nw", "NorthWest"], ["rh-se", "SouthEast"], ["rh-sw", "SouthWest"],
  ];
  const box = $("resize-handles");
  for (const [cls, dir] of dirs) {
    const h = document.createElement("div");
    h.className = cls;
    h.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      void win.startResizeDragging(dir as never);
    });
    box.appendChild(h);
  }
}

/** Reflète la session active dans la barre de titre (utile + évite le doublon). */
function setTitlebar() {
  const s = state.active === null ? null : state.sessions.get(state.active);
  $("tb-name").textContent = s && !s.closed ? `${s.alias} — Avash` : "Avash";
}


// ---------- Mise à jour ----------

let updateBusy = false;
async function checkForUpdates() {
  if (updateBusy) return;
  updateBusy = true;
  const ver = $("app-version");
  const prev = ver.textContent;
  ver.textContent = "…";
  try {
    const update = await checkUpdate();
    if (!update) {
      ver.textContent = "à jour";
      setTimeout(() => (ver.textContent = prev), 1800);
      return;
    }
    ver.textContent = prev;
    const ok = await askConfirm(
      `Version ${update.version} disponible (actuelle : ${update.currentVersion}).\n\n` +
        `${update.body ?? ""}\n\nTélécharger et installer maintenant ?`,
      { danger: false, ok: "Installer" },
    );
    if (!ok) return;
    await update.downloadAndInstall();
    if (await askConfirm("Mise à jour installée. Redémarrer Avash maintenant ?", { danger: false, ok: "Redémarrer" })) await relaunch();
  } catch (e) {
    // Endpoint injoignable / pas encore configuré / hors ligne : on le dit
    // sans dramatiser.
    ver.textContent = prev;
    notifyErreur(`Vérification des mises à jour impossible : ${e}`);
  } finally {
    updateBusy = false;
  }
}
$("app-version").addEventListener("click", checkForUpdates);

// ---------- RDP (bureau distant, via le sidecar avash-rdp) ----------


type RdpTarget = { host: string; port: number | null; user: string; password: string; width?: number; height?: number; hostId?: string; name?: string };

type RdpHostT = { id: string; name: string; host: string; port: number; user: string; width: number; height: number; folder: string };
let rdpHostsList: RdpHostT[] = [];
const RDP_ACK = new Uint8Array([6]); // accusé de rendu (cadencement adaptatif)

// Presse-papiers poste -> bureau distant (CLIPRDR). On lit le presse-papiers
// local et on l'annonce à la session RDP active quand Avash reprend le focus
// (tu copies ailleurs, tu reviens, tu colles dans le distant). Message [8].
let lastClipText = "";

async function pushLocalClipboard(force = false): Promise<void> {
  if (!partageClipboard()) return;
  if (state.active === null || !rdpSessions.has(state.active)) return;
  let text: string;
  try {
    text = (await clipReadText()) ?? "";
  } catch {
    return; // pas de texte (image/fichier) ou accès refusé
  }
  if (!text) return;
  if (!force && text === lastClipText) return; // au switch d'onglet, on renvoie quand même
  lastClipText = text;
  const s = rdpSessions.get(state.active);
  if (s?.ws && s.ws.readyState === WebSocket.OPEN) {
    const body = new TextEncoder().encode(text);
    const msg = new Uint8Array(1 + body.length);
    msg[0] = 8;
    msg.set(body, 1);
    s.ws.send(msg);
  }
}
// Le presse-papiers local n'est PAS poussé au simple retour de la fenêtre : cela
// envoyait son contenu — souvent un mot de passe fraîchement copié — à tout
// serveur RDP ouvert, sans le moindre geste de l'utilisateur, et à chaque
// bascule de fenêtre. Il ne part plus que sur un collage explicite (Ctrl+V) ou
// quand le serveur le réclame, dans l'onglet actif.
const rdpSessions = new Map<number, { canvas: HTMLCanvasElement; tab: HTMLElement; ws: WebSocket | null; ro?: ResizeObserver; hostId?: string; syncSize?: () => void; target?: RdpTarget }>();

async function openRdp(t: RdpTarget) {
  const id = state.nextId++;
  // Onglet
  const tabs = $("tabs");
  tabs.querySelector(".no-session")?.remove();
  const tab = document.createElement("div");
  tab.className = "tab active";
  tab.innerHTML = `<span class="state connecting"></span><span class="label"></span><span class="close"></span>`;
  // Même règle que les onglets SSH : le nom de l'hôte enregistré, et à défaut
  // « utilisateur@adresse » pour une connexion directe. Les deux protocoles se
  // lisent ainsi de la même façon dans la barre d'onglets.
  tab.querySelector(".label")!.textContent = t.name ?? `${t.user}@${t.host}`;
  tab.querySelector(".close")!.innerHTML = ic("x");
  tabs.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
  tabs.appendChild(tab);

  // Résolution = taille de la zone disponible d'Avash (adaptatif), sauf si
  // une taille précise est imposée. RDP : largeur paire, bornes 200..8192.
  const area = $("terminal").getBoundingClientRect();
  const even = (n: number) => n - (n % 2);
  // Mutables : au redimensionnement natif, le serveur renvoie la vraie taille
  // (message CONNECTED) et on les remet à jour — le mappage souris suit.
  let rdpW = Math.max(200, Math.min(8192, even(Math.round(t.width || area.width || 1280))));
  let rdpH = Math.max(200, Math.min(8192, Math.round(t.height || area.height || 800)));

  // Canvas dans la zone terminal
  $("terminal-empty").style.display = "none";
  const wrap = document.createElement("div");
  wrap.className = "rdp-container";
  const canvas = document.createElement("canvas");
  canvas.width = rdpW;
  canvas.height = rdpH;
  canvas.tabIndex = 0;
  // Indicateur de qualité en direct (fps / débit / latence). Clic pour masquer.
  const hud = document.createElement("div");
  hud.className = "rdp-hud";
  hud.title = "Qualité de la session — clic pour masquer";
  hud.addEventListener("click", () => hud.classList.toggle("mini"));
  wrap.appendChild(canvas);
  wrap.appendChild(hud);
  $("terminal").appendChild(wrap);
  const ctx = canvas.getContext("2d")!;
  rdpSessions.set(id, { canvas, tab, ws: null, hostId: t.hostId, target: t });
  state.active = id;

  tab.addEventListener("click", () => focusRdp(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => { e.stopPropagation(); closeRdp(id); });

  // Souris/clavier → sidecar via le WebSocket (binaire). Ignore si non prêt.
  const send = (bytes: number[]) => {
    const s = rdpSessions.get(id);
    if (s?.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array(bytes));
  };
  // Mappage souris -> pixels du bureau (letterbox object-fit:contain), testé.
  const pos = (e: MouseEvent): [number, number] =>
    rdpMousePos(e.clientX, e.clientY, canvas.getBoundingClientRect(), rdpW, rdpH);
  // Mouvements souris throttlés au rAF : un seul paquet par frame d'affichage.
  let moveX = 0, moveY = 0, movePending = false;
  canvas.addEventListener("mousemove", (e) => {
    [moveX, moveY] = pos(e);
    if (movePending) return;
    movePending = true;
    requestAnimationFrame(() => { movePending = false; send([1, ...le16(moveX), ...le16(moveY)]); });
  });
  canvas.addEventListener("mousedown", (e) => { e.preventDefault(); canvas.focus(); const [x, y] = pos(e); send([2, e.button, 1, ...le16(x), ...le16(y)]); });
  canvas.addEventListener("mouseup", (e) => { const [x, y] = pos(e); send([2, e.button, 0, ...le16(x), ...le16(y)]); });
  // Clic droit : uniquement pour le bureau distant. On empêche le menu du
  // navigateur ET la remontée vers #terminal (qui ouvrirait le menu d'Avash).
  canvas.addEventListener("contextmenu", (e) => { e.preventDefault(); e.stopPropagation(); });
  canvas.addEventListener("wheel", (e) => { e.preventDefault(); const d = e.deltaY > 0 ? -120 : 120; send([3, ...le16(d & 0xffff), 0, 0, 0, 0]); });
  canvas.addEventListener("keydown", (e) => {
    if (e.code === "F11") { e.preventDefault(); return; } // géré globalement (plein écran)
    e.preventDefault();
    // Pas de resynchronisation ici : le navigateur ne sait pas lire ces verrous
    // sous WebKitGTK, et renvoyer sa valeur éteindrait le pavé numérique du
    // distant dès la première frappe. Verr.Num est de toute façon transmise
    // comme n'importe quelle touche : le bureau distant bascule lui-même.
    const sc = rdpScancode(e.code); if (sc) send([4, ...le16(sc), 1]);
  });
  canvas.addEventListener("keyup", (e) => { e.preventDefault(); const sc = rdpScancode(e.code); if (sc) send([4, ...le16(sc), 0]); });
  // Focus du bureau distant = l'utilisateur va sans doute coller : on lui pousse
  // le presse-papiers local à jour (fiabilise le collage local->distant).
  canvas.addEventListener("focus", () => {
    void currentLocks().then((l) => { if (l !== null) send([10, l]); });
    void pushLocalClipboard(true);
  });

  // Redimensionnement NATIF du bureau distant : quand la zone Avash change, on
  // demande au serveur de re-rendre à la nouvelle taille (Display Control DVC).
  // Débounce pour ne pas spammer pendant le glissé de la fenêtre. Message [5].
  let resizeTimer: number | undefined;
  let resizeInFlight = false; // une seule renégociation RDP à la fois
  let resizeGuard: number | undefined;
  const sendResize = () => {
    if (state.active !== id) return; // seul le bureau visible se redimensionne
    const a = $("terminal").getBoundingClientRect();
    const w = Math.max(200, Math.min(8192, even(Math.round(a.width))));
    const h = Math.max(200, Math.min(8192, Math.round(a.height)));
    if (Math.abs(w - rdpW) < 8 && Math.abs(h - rdpH) < 8) return; // négligeable
    if (resizeInFlight) return; // on rejouera la taille finale à la fin (kind 1)
    resizeInFlight = true;
    window.clearTimeout(resizeGuard);
    resizeGuard = window.setTimeout(() => { resizeInFlight = false; }, 3000); // filet
    send([5, ...le16(w), ...le16(h)]);
  };
  const ro = new ResizeObserver(() => {
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(sendResize, 400);
  });
  ro.observe($("terminal"));
  rdpSessions.get(id)!.ro = ro;
  rdpSessions.get(id)!.syncSize = sendResize;

  // Bureau reçu via WebSocket local BINAIRE (ArrayBuffer natif : ni base64 ni
  // JSON — débit maximal, même en 3440×1440).
  //   [1] CONNECTED w,h · [2] FRAME x,y,w,h + RGBA · [3] ERROR utf8
  try {
    const conn = await invoke<{ port: number; token: string }>("rdp_open", {
      id, host: t.host, port: t.port, user: t.user, password: t.password,
      width: rdpW, height: rdpH,
    });
    // L'onglet a pu être fermé pendant la connexion (TLS + NLA prennent du
    // temps) : sans cette garde, l'affectation levait une exception, attrapée
    // plus bas et présentée comme un échec de connexion alors que l'utilisateur
    // venait simplement de fermer.
    const session = rdpSessions.get(id);
    if (!session) { void invoke("rdp_close", { id }).catch(() => {}); return; }
    const ws = new WebSocket(`ws://127.0.0.1:${conn.port}`);
    ws.binaryType = "arraybuffer";
    session.ws = ws;
    ws.onopen = () => {
      ws.send(new TextEncoder().encode(conn.token));
      annoncerPartageClip(ws);
      // Annonce initiale du presse-papiers local au bureau distant.
      window.setTimeout(() => void pushLocalClipboard(), 600);
    };
    ws.onmessage = (ev) => {
      if (!rdpSessions.has(id)) return;
      const buf = ev.data as ArrayBuffer;
      const dv = new DataView(buf);
      const kind = dv.getUint8(0);
      if (kind === 2) {
        try {
          const x = dv.getUint16(1, true), y = dv.getUint16(3, true);
          const fw = dv.getUint16(5, true), fh = dv.getUint16(7, true);
          ctx.putImageData(new ImageData(new Uint8ClampedArray(buf, 9, fw * fh * 4), fw, fh), x, y);
        } catch (err) {
          console.warn("frame RDP invalide", err);
        }
        // ACK de rendu (même si la frame était invalide, pour ne pas figer le flux).
        if (ws.readyState === WebSocket.OPEN) ws.send(RDP_ACK);
      } else if (kind === 7) {
        const fps = dv.getUint16(1, true);
        const kbps = dv.getUint32(3, true);
        const lat = dv.getUint16(7, true);
        const q = lat < 40 ? "q-ok" : lat < 100 ? "q-mid" : "q-bad";
        const rate = kbps >= 1024 ? `${(kbps / 1024).toFixed(1)} Mo/s` : `${kbps} Ko/s`;
        hud.innerHTML = `<b>${fps}</b> fps · ${rate} · <span class="${q}">${lat} ms</span>`;
      } else if (kind === 1) {
        // Changer la taille du canvas l'efface : on capture l'image courante et
        // on la réétire dans la nouvelle taille, le temps que le serveur renvoie
        // une image complète. Plus de flash noir pendant la renégociation.
        const nw = dv.getUint16(1, true), nh = dv.getUint16(3, true);
        let snap: HTMLCanvasElement | null = null;
        if (canvas.width > 0 && canvas.height > 0) {
          snap = document.createElement("canvas");
          snap.width = canvas.width;
          snap.height = canvas.height;
          snap.getContext("2d")!.drawImage(canvas, 0, 0);
        }
        rdpW = nw;
        rdpH = nh;
        canvas.width = rdpW;
        canvas.height = rdpH;
        if (snap) ctx.drawImage(snap, 0, 0, rdpW, rdpH);
        tab.querySelector(".state")!.className = "state live";
        // Aligner les verrous du bureau distant sur ceux du poste.
        void currentLocks().then((l) => { if (l !== null) send([10, l]); });
        // Renégociation terminée : si la fenêtre a encore bougé entre-temps, on
        // applique la taille finale (une seule fois, évite les cascades).
        resizeInFlight = false;
        window.clearTimeout(resizeGuard);
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(sendResize, 120);
      } else if (kind === 8) {
        // Le bureau distant a copié du texte -> presse-papiers du poste. Le
        // réglage vaut dans les deux sens : sans cela, un bureau hostile
        // remplaçait en boucle le presse-papiers local — on copie une commande
        // depuis sa documentation, on colle dans son terminal, on exécute la
        // sienne — et ce, même après avoir explicitement coupé le partage.
        if (!partageClipboard()) return;
        const text = new TextDecoder().decode(new Uint8Array(buf, 1));
        lastClipText = text; // ne pas le renvoyer aussitôt au distant
        clipWriteText(text).catch(() => {});
      } else if (kind === 3) {
        tab.querySelector(".state")!.className = "state closed";
        notifyErreur(`RDP : ${new TextDecoder().decode(new Uint8Array(buf, 1))}`);
      }
    };
    ws.onclose = () => {
      const st = tab.querySelector(".state");
      if (st) st.className = "state closed";
      tab.classList.add("dead");
      // Le processus RDP et l'observateur de taille survivaient à la coupure :
      // le premier restait dans la table côté Rust jusqu'à l'arrêt de
      // l'application, le second continuait d'observer #terminal pour un
      // canvas mort. L'onglet et le canvas restent, eux — « Reconnecter »
      // doit rester possible.
      rdpSessions.get(id)?.ro?.disconnect();
      void invoke("rdp_close", { id }).catch(() => {});
      showRdpClosed(id);
    };
    ws.onerror = () => { /* onclose suivra */ };
  } catch (e) {
    // Fermeture volontaire pendant la connexion : le back le signale par un
    // marqueur. Rien à afficher, l'onglet n'existe déjà plus.
    if (String(e).includes("[AVASH_RDP_ANNULE]")) return;
    if (!rdpSessions.has(id)) return;
    tab.querySelector(".state")!.className = "state closed";
    notify(`Connexion RDP impossible : ${e}`, "erreur");
    showRdpClosed(id); // proposer de réessayer
  }
  focusRdp(id);
}

/** Dit au sidecar si son bureau est visible. Message [11], 1 = en pause.
 *
 *  Un onglet masqué continuait d'accuser réception de chaque trame : le sidecar
 *  y voyait la voie libre et poussait sans relâche des images entières — 8 Mo
 *  par trame en 1080p — vers un canvas invisible. Deux bureaux ouverts
 *  doublaient donc le travail utile sans rien afficher de plus. */
/** Annonce au sidecar si le partage de presse-papiers est autorisé. Message [12].
 *
 *  Sans cela le sidecar réclamait au serveur le contenu de son presse-papiers à
 *  chaque annonce de copie, même quand l'interface n'avait plus le droit de
 *  l'appliquer : du trafic et une lecture inutiles. */
function annoncerPartageClip(ws: WebSocket): void {
  if (ws.readyState === WebSocket.OPEN) ws.send(new Uint8Array([12, partageClipboard() ? 1 : 0]));
}

function marquerVisibilite(s: { ws: WebSocket | null }, visible: boolean): void {
  if (s.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array([11, visible ? 0 : 1]));
}

function focusRdp(id: number) {
  state.active = id;
  for (const [sid, s] of rdpSessions) {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    (s.canvas.parentElement as HTMLElement).style.display = active ? "flex" : "none";
    marquerVisibilite(s, active);
    if (active) {
      s.canvas.focus();
      // La session inactive n'a pas suivi les redimensionnements de la fenêtre
      // (seule l'active se resize) : on rattrape sa taille en devenant active.
      s.syncSize?.();
      // Un canvas caché peut avoir perdu son contenu (backing-store WebKitGTK) :
      // on demande au sidecar de renvoyer l'image entière. Message [9].
      if (s.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array([9]));
    }
  }
  // Masquer les terminaux PTY.
  state.sessions.forEach((s) => { (s.term.element?.parentElement as HTMLElement).style.display = "none"; s.tab.classList.remove("active"); });
  $("terminal-empty").style.display = "none";
  // Le switch d'onglet ne déclenche pas l'événement focus fenêtre : on renvoie
  // explicitement le presse-papiers local à la session qui devient active,
  // sinon le collage local->distant ne marche pas après un changement d'onglet.
  void pushLocalClipboard(true);
  renderHosts(); // met à jour le surlignage « sélectionné »
}

function closeRdp(id: number) {
  const s = rdpSessions.get(id);
  if (!s) return;
  if (document.body.classList.contains("rdp-full")) {
    document.body.classList.remove("rdp-full");
    getCurrentWindow().setFullscreen(false).catch(() => {});
  }
  s.ro?.disconnect();
  s.ws?.close();
  invoke("rdp_close", { id }).catch(() => {});
  s.canvas.parentElement?.remove();
  s.tab.remove();
  rdpSessions.delete(id);
  if (state.active === id) {
    // Même défaut en miroir : `focusRdp` masque tous les terminaux SSH, et
    // fermer le bureau actif laissait la zone centrale vide alors qu'une
    // session SSH restait ouverte dans la barre d'onglets.
    const suivant = orderedTabs().find((t) => !(t.kind === "rdp" && t.id === id));
    state.active = null;
    if (suivant) {
      focusTab(suivant);
    } else {
      $("terminal-empty").style.display = "flex";
    }
  }
  renderHosts(); // éteint le voyant vert de l'hôte fermé
}

/** Bureau RDP fermé (serveur/réseau) : propose de reconnecter ou fermer l'onglet
 *  — équivalent du message « Entrée : reconnecter · Ctrl+W : fermer » du SSH. */
function showRdpClosed(id: number) {
  const s = rdpSessions.get(id);
  if (!s) return; // fermeture volontaire (l'onglet est déjà retiré)
  const wrap = s.canvas.parentElement as HTMLElement | null;
  if (!wrap || wrap.querySelector(".rdp-closed")) return;
  const ov = document.createElement("div");
  ov.className = "rdp-closed";
  ov.innerHTML =
    `<div class="rdp-closed-box"><p>Connexion RDP fermée.</p>` +
    `<div class="rdp-closed-actions">` +
    `<button type="button" class="btn-primary" data-act="reconnect">Reconnecter</button>` +
    `<button type="button" class="btn-ghost" data-act="close">Fermer l'onglet</button>` +
    `</div></div>`;
  ov.querySelector('[data-act="reconnect"]')!.addEventListener("click", () => {
    const t = s.target;
    closeRdp(id);
    if (t) void openRdp(t);
  });
  ov.querySelector('[data-act="close"]')!.addEventListener("click", () => closeRdp(id));
  wrap.appendChild(ov);
}

/** Connexion à un bureau RDP enregistré (mot de passe du trousseau, sinon demandé). */
async function connectRdpSaved(h: RdpHostT) {
  // On demande au cœur s'il connaît ce compte, sans jamais rapatrier le secret :
  // un mot de passe vide indique à `rdp_open` de le lire lui-même dans le
  // trousseau. Il ne traverse donc pas l'IPC et ne séjourne pas dans le tas de
  // la webview pour toute la durée de l'onglet.
  const connu = await invoke<boolean>("rdp_password_known", { host: h.host, port: h.port, user: h.user }).catch(() => false);
  let pw = "";
  if (!connu) {
    const rep = await askPassword(`${h.user}@${h.host}:${h.port}`);
    if (!rep) return;
    pw = rep.password;
    if (rep.remember && pw) {
      const memorise = await invoke("rdp_password_save", { host: h.host, port: h.port, user: h.user, password: pw })
        .then(() => true)
        .catch(() => false);
      // Une fois au trousseau, le secret n'a plus aucune raison de continuer sa
      // route : `rdp_open` le relira côté natif. Il séjournait sinon dans
      // `rdpSessions[id].target.password` toute la vie de l'onglet — le
      // confinement ne valait donc que pour un bureau déjà mémorisé, pas pour
      // la connexion où l'on coche « mémoriser ».
      if (memorise) pw = "";
    }
  }
  await openRdp({ host: h.host, port: h.port, user: h.user, password: pw, hostId: h.id, name: h.name });
}

function openRdpMenu(h: RdpHostT, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("rdp-context");
  m.dataset.id = h.id;
  placerMenu(m, e);
  m.classList.add("open");
}
window.addEventListener("click", () => $("rdp-context").classList.remove("open"));
$("rdp-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const id = $("rdp-context").dataset.id;
  $("rdp-context").classList.remove("open");
  const h = rdpHostsList.find((x) => x.id === id);
  if (!act || !h) return;
  if (act === "connect") void connectRdpSaved(h);
  else if (act === "edit") openEditRdp(h);
  else if (act === "move") openMoveModal("rdp", h.id);
  else if (act === "forget") {
    // cf. le volet SSH : une action muette ne se distingue pas d'un clic raté.
    await invoke("rdp_password_forget", { host: h.host, port: h.port, user: h.user })
      .then(() => notify(`Mot de passe oublié pour ${h.name}.`, "succes"))
      .catch((err) => notifyErreur(`Le mot de passe n'a pas pu être oublié : ${err}`));
  } else if (act === "delete") {
    if (!(await askConfirm(`Supprimer le bureau RDP « ${h.name} » ?\n\nSon mot de passe mémorisé sera aussi oublié.`))) return;
    await invoke("rdp_host_delete", { id: h.id }).catch((err) => notifyErreur(`Suppression impossible : ${err}`));
    await loadHosts();
  }
});

/** Ouvre la modale d'édition d'un bureau RDP enregistré, pré-remplie. */
function openEditRdp(h: RdpHostT) {
  $("re-error").hidden = true;
  const f = $("rdp-edit-form") as HTMLFormElement;
  f.dataset.oldHost = h.host;
  f.dataset.oldPort = String(h.port);
  f.dataset.oldUser = h.user;
  ($("re-id") as HTMLInputElement).value = h.id;
  ($("re-name") as HTMLInputElement).value = h.name;
  ($("re-addr") as HTMLInputElement).value = h.host;
  ($("re-port") as HTMLInputElement).value = String(h.port);
  ($("re-user") as HTMLInputElement).value = h.user;
  ($("re-password") as HTMLInputElement).value = "";
  ($("rdp-edit-form") as HTMLFormElement).dataset.folder = h.folder ?? "";
  $("rdp-edit-modal").classList.add("open");
  setTimeout(() => ($("re-name") as HTMLInputElement).focus(), 30);
}

function closeEditRdp() {
  $("rdp-edit-modal").classList.remove("open");
}

$("re-cancel").addEventListener("click", closeEditRdp);
$("rdp-edit-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const err = $("re-error");
  const submit = $("re-submit") as HTMLButtonElement;
  const f = $("rdp-edit-form") as HTMLFormElement;
  const name = val("re-name");
  const host = val("re-addr");
  const user = val("re-user");
  const portRaw = val("re-port");
  const port = portRaw ? Number(portRaw) : 3389;
  const pw = ($("re-password") as HTMLInputElement).value;
  if (!name || !host || !user) {
    err.textContent = "Nom, adresse et utilisateur requis.";
    err.hidden = false;
    return;
  }
  submit.disabled = true;
  try {
    await invoke("rdp_host_save", { id: val("re-id"), name, host, port, user, width: 0, height: 0, folder: ($("rdp-edit-form") as HTMLFormElement).dataset.folder ?? null });
    // Le compte du trousseau dépend de host/port/user : si l'un change, on
    // migre (ou remplace) le mot de passe mémorisé vers le nouveau compte.
    const oldHost = f.dataset.oldHost ?? host;
    const oldPort = Number(f.dataset.oldPort ?? String(port));
    const oldUser = f.dataset.oldUser ?? user;
    const accountChanged = oldHost !== host || oldPort !== port || oldUser !== user;
    if (pw) {
      await invoke("rdp_password_save", { host, port, user, password: pw }).catch(() => {});
      if (accountChanged) {
        await invoke("rdp_password_forget", { host: oldHost, port: oldPort, user: oldUser }).catch(() => {});
      }
    } else if (accountChanged) {
      // Migration confiée au cœur : le secret n'a aucune raison de faire
      // l'aller-retour par l'interface pour changer de clé de trousseau.
      await invoke("rdp_password_move", { oldHost, oldPort, oldUser, host, port, user }).catch(() => {});
    }
    closeEditRdp();
    await loadHosts();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});

/** Plein écran du bureau RDP : fenêtre en plein écran + châssis masqué. */
async function toggleRdpFullscreen() {
  // N'a de sens que sur un onglet RDP.
  if (state.active === null || !rdpSessions.has(state.active)) return;
  const full = !document.body.classList.contains("rdp-full");
  document.body.classList.toggle("rdp-full", full);
  try { await getCurrentWindow().setFullscreen(full); } catch { /* */ }
  const s = state.active !== null ? rdpSessions.get(state.active) : null;
  s?.canvas.focus();
}
window.addEventListener("keydown", (e) => {
  if (e.key === "F11") { e.preventDefault(); void toggleRdpFullscreen(); }
});

/** Table minimale code clavier → scancode PC (set 1). Suffisant pour saisir. */

// ---------- Panneaux redimensionnables (barre latérale + SFTP) ----------

/** Ajuste le terminal actif à la nouvelle taille (le canvas RDP suit en CSS). */
function fitActive() {
  if (state.active === null) return;
  state.sessions.get(state.active)?.fit.fit();
}

const root = document.documentElement;
function loadPanelPrefs() {
  try {
    const sw = localStorage.getItem("avash.side.w");
    if (sw) root.style.setProperty("--side-w", `${clampSide(Number(sw))}px`);
    const fw = localStorage.getItem("avash.sftp.w");
    if (fw) root.style.setProperty("--sftp-w", `${clampSftp(Number(fw))}px`);
    if (localStorage.getItem("avash.side.collapsed") === "1") document.body.classList.add("side-collapsed");
  } catch { /* stockage indispo */ }
}
const clampSide = (n: number) => Math.max(180, Math.min(460, n));
const clampSftp = (n: number) => Math.max(220, Math.min(640, n));

/**
 * Rend une poignée glissable. `compute(dx, startW)` donne la nouvelle largeur ;
 * le rendu est throttlé au rAF et ré-ajuste le terminal actif à chaque frame.
 */
function attachResizer(
  handle: HTMLElement,
  target: HTMLElement,
  cssVar: string,
  storeKey: string,
  clamp: (n: number) => number,
  compute: (dx: number, startW: number) => number,
) {
  handle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = target.getBoundingClientRect().width;
    let pending = startW;
    let raf = 0;
    document.body.classList.add("resizing");
    const onMove = (ev: MouseEvent) => {
      pending = clamp(compute(ev.clientX - startX, startW));
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        root.style.setProperty(cssVar, `${pending}px`);
        fitActive();
      });
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      if (raf) cancelAnimationFrame(raf);
      document.body.classList.remove("resizing");
      root.style.setProperty(cssVar, `${pending}px`);
      try { localStorage.setItem(storeKey, String(Math.round(pending))); } catch { /* */ }
      fitActive();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  // Double-clic : réinitialise à la largeur par défaut.
  handle.addEventListener("dblclick", () => {
    root.style.removeProperty(cssVar);
    try { localStorage.removeItem(storeKey); } catch { /* */ }
    fitActive();
  });
}

function initPanels() {
  loadPanelPrefs();
  // Barre latérale : glisser le bord droit élargit.
  attachResizer($("side-resize"), document.querySelector(".sidebar") as HTMLElement,
    "--side-w", "avash.side.w", clampSide, (dx, w) => w + dx);
  // Panneau SFTP : bord GAUCHE, glisser vers la gauche élargit.
  attachResizer($("sftp-resize"), $("sftp-panel"), "--sftp-w", "avash.sftp.w", clampSftp, (dx, w) => w - dx);

  const toggleSidebar = () => {
    const collapsed = document.body.classList.toggle("side-collapsed");
    try { localStorage.setItem("avash.side.collapsed", collapsed ? "1" : "0"); } catch { /* */ }
    // Le repli change la largeur sans event resize : on ajuste à la fin.
    setTimeout(fitActive, 0);
  };
  $("side-toggle").addEventListener("click", toggleSidebar);
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === ".") { e.preventDefault(); toggleSidebar(); }
  });
}
initPanels();


// ---------- Gestion des dossiers (création, menu, déplacement) ----------

/** Ensemble des dossiers connus (registre + dérivés des hôtes), triés. */
function allFolders(): string[] {
  const set = new Set<string>(state.folders);
  const add = (f: string) => {
    let acc = "";
    for (const seg of (f || "").split("/").filter(Boolean)) {
      acc = acc ? `${acc}/${seg}` : seg;
      set.add(acc);
    }
  };
  for (const h of state.hosts) add(h.folder ?? "");
  for (const h of rdpHostsList) add(h.folder ?? "");
  return [...set].filter(Boolean).sort();
}

async function createFolder(parent: string) {
  const name = await askText(
    parent ? "Nouveau sous-dossier" : "Nouveau dossier",
    parent ? `Dans \u00ab ${parent} \u00bb` : "Nom du dossier",
    "",
  );
  if (!name || !name.trim()) return;
  const path = parent ? `${parent}/${name.trim()}` : name.trim();
  try {
    await invoke("folder_create", { path });
    collapsedFolders.delete(parent);
    saveCollapsed();
    await loadHosts();
  } catch (e) {
    notifyErreur(`Cr\u00e9ation impossible : ${e}`);
  }
}

$("new-folder-btn").addEventListener("click", () => void createFolder(""));

function openFolderMenu(path: string, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("folder-context");
  m.dataset.path = path;
  placerMenu(m, e);
  m.classList.add("open");
}
window.addEventListener("click", () => $("folder-context").classList.remove("open"));
$("folder-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const path = $("folder-context").dataset.path ?? "";
  $("folder-context").classList.remove("open");
  if (!act || !path) return;
  if (act === "new") {
    await createFolder(path);
  } else if (act === "rename") {
    const leaf = path.split("/").pop() ?? path;
    const name = await askText("Renommer le dossier", `Nouveau nom de \u00ab ${leaf} \u00bb`, leaf);
    if (!name || !name.trim() || name.trim() === leaf) return;
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    const to = parent ? `${parent}/${name.trim()}` : name.trim();
    try {
      await invoke("folder_rename", { from: path, to });
      await loadHosts();
    } catch (ex) {
      notifyErreur(`Renommage impossible : ${ex}`);
    }
  } else if (act === "delete") {
    if (!(await askConfirm(`Supprimer le dossier \u00ab ${path} \u00bb ?\n\nLes h\u00f4tes qu'il contient (et ses sous-dossiers) reviennent \u00e0 la racine ; ils ne sont pas supprim\u00e9s.`))) return;
    try {
      await invoke("folder_delete", { path });
      await loadHosts();
    } catch (ex) {
      notifyErreur(`Suppression impossible : ${ex}`);
    }
  }
});

let moveTarget: { kind: string; id: string } | null = null;
function openMoveModal(kind: string, id: string) {
  moveTarget = { kind, id };
  $("move-error").hidden = true;
  ($("move-new") as HTMLInputElement).value = "";
  const listEl = $("move-list");
  listEl.innerHTML = "";
  const addRow = (label: string, folder: string) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "move-row";
    row.textContent = label;
    row.addEventListener("click", () => void doMove(folder));
    listEl.appendChild(row);
  };
  addRow("\u2196 Racine", "");
  for (const f of allFolders()) addRow(f, f);
  $("move-modal").classList.add("open");
  setTimeout(() => ($("move-new") as HTMLInputElement).focus(), 30);
}
function closeMoveModal() {
  $("move-modal").classList.remove("open");
  moveTarget = null;
}
async function doMove(folder: string) {
  if (!moveTarget) return;
  await moveHostTo(moveTarget.kind, moveTarget.id, folder);
  closeMoveModal();
}
$("move-cancel").addEventListener("click", closeMoveModal);
$("move-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = ($("move-new") as HTMLInputElement).value.trim();
  if (!f) {
    $("move-error").textContent = "Saisis un nom de dossier.";
    $("move-error").hidden = false;
    return;
  }
  void doMove(f);
});

// Racine : d\u00e9poser un h\u00f4te sur la zone vide de la liste le remet \u00e0 la racine.
setupFolderDrop($("host-list"), "", false);

