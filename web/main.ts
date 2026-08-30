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
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ic, fileIconName, hydrateIcons } from "./icons";
import {
  humanSize, filterHosts, allTags, remoteJoin, parentDir, isPasswordRequired, isHostKeyChanged, stripHtml, hostInitials, hostHue, osBadge,
  sortSftpEntries, shortDate, shellQuote, validFileName, snippetPreview, snippetVars, renderSnippet, type SftpEntry, type Snippet,
  describeTunnel, tunnelFlag, tunnelTraffic, activeTunnelsByHost,
  type Host, type TunnelDef, type TunnelStatus, type TunnelKind, type OsInfo,
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
type TreeNode = { name: string; path: string; children: Map<string, TreeNode>; items: TreeItem[] };

function newNode(name: string, path: string): TreeNode {
  return { name, path, children: new Map(), items: [] };
}

/** Descend (en créant au besoin) jusqu'au nœud du chemin donné. */
function ensureFolder(root: TreeNode, path: string): TreeNode {
  let node = root;
  let acc = "";
  for (const seg of (path || "").split("/").filter(Boolean)) {
    acc = acc ? `${acc}/${seg}` : seg;
    let child = node.children.get(seg);
    if (!child) {
      child = newNode(seg, acc);
      node.children.set(seg, child);
    }
    node = child;
  }
  return node;
}

/** Construit l'arbre à partir du registre de dossiers + des hôtes SSH et RDP. */
function buildTree(): TreeNode {
  const root = newNode("", "");
  for (const f of state.folders) ensureFolder(root, f);
  for (const h of state.hosts) ensureFolder(root, h.folder ?? "").items.push({ kind: "ssh", ssh: h });
  for (const h of rdpHostsList) ensureFolder(root, h.folder ?? "").items.push({ kind: "rdp", rdp: h });
  return root;
}

function nodeCount(node: TreeNode): number {
  let n = node.items.length;
  for (const c of node.children.values()) n += nodeCount(c);
  return n;
}

/** Une ligne d'hôte SSH (avatar, logo distro, tags, état), déplaçable. */
function sshHostElement(h: Host): HTMLElement {
  const el = document.createElement("div");
  const selected = state.active !== null && state.sessions.get(state.active)?.alias === h.alias;
  el.className = "host" + (selected ? " selected" : "");
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
    el.title = `${os.pretty} — double-clic : connexion, clic droit : options`;
  } else {
    ini.textContent = hostInitials(h.alias);
  }
  el.querySelector(".alias")!.textContent = h.alias;
  el.querySelector(".meta")!.textContent = target;
  if (h.tags.length > 0) {
    const chips = document.createElement("span");
    chips.className = "host-tags";
    for (const t of h.tags.slice(0, 3)) {
      const c = document.createElement("span");
      c.className = "host-tag" + (t === state.tagFilter ? " on" : "");
      c.textContent = t;
      c.title = `Filtrer par « ${t} »`;
      c.addEventListener("click", (ev) => {
        ev.stopPropagation();
        state.tagFilter = state.tagFilter === t ? null : t;
        renderHosts();
      });
      chips.appendChild(c);
    }
    el.querySelector(".info")!.appendChild(chips);
  }
  const dot = el.querySelector(".dot") as HTMLElement;
  dot.className = "dot " + hostSessionState(h.alias);
  if (h.alias === state.pickedAlias) el.classList.add("picked");
  if (!os) el.title = "Double-clic : connexion — clic droit : options";
  el.addEventListener("click", () => {
    state.pickedAlias = h.alias;
    for (const n of $("host-list").querySelectorAll(".host.picked")) n.classList.remove("picked");
    el.classList.add("picked");
  });
  el.addEventListener("dblclick", () => openSession(h));
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openHostMenu(h, e as MouseEvent);
  });
  makeHostDraggable(el, "ssh", h.alias);
  return el;
}

/** Une ligne de bureau RDP enregistré, déplaçable. */
function rdpHostElement(h: RdpHostT): HTMLElement {
  const el = document.createElement("div");
  el.className = "host";
  el.innerHTML = `<span class="avatar rdp"><span class="ini logo"></span></span><span class="info"><div class="alias"></div><div class="meta"></div></span>`;
  (el.querySelector(".ini") as HTMLElement).innerHTML = ic("monitor");
  el.querySelector(".alias")!.textContent = h.name;
  el.querySelector(".meta")!.textContent = `${h.user}@${h.host}:${h.port}`;
  el.title = "Double-clic : connexion RDP — clic droit : options";
  el.addEventListener("dblclick", () => connectRdpSaved(h));
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openRdpMenu(h, e as MouseEvent);
  });
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
    alert(`Déplacement impossible : ${e}`);
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
  row.querySelector(".fcount")!.textContent = String(nodeCount(node));
  row.title = `${node.path} — clic : plier/déplier, clic droit : options, déposer un hôte pour le ranger`;
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
  list.innerHTML = "";
  renderTagBar();
  const q = state.filter.trim().toLowerCase();
  const filtering = q !== "" || state.tagFilter !== null;
  const sshShown = filterHosts(state.hosts, state.filter, state.tagFilter);
  const rdpShown = rdpHostsList.filter(
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
    empty.innerHTML = `<p>Aucun hôte ne correspond à « ${stripHtml(state.filter)} ».</p>`;
    list.appendChild(empty);
  }
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
      if (data === "\r" && s.reconnect) s.reconnect();
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
        const ok = confirm(`${clean}\n\nOublier l'ancienne clé et réessayer ? (à ne faire que si le changement est légitime)`);
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
        markClosed(s, `⚔️ Échec connexion : ${msg}`);
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
  markClosed(s, "⚔️ Trois tentatives échouées.");
}

/**
 * Marque un onglet termine et explique quoi faire : sans cette ligne,
 * l'utilisateur ne sait pas si ca charge encore ni comment relancer.
 */
function markClosed(s: Session, why: string) {
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
      markClosed(session, `⚔️ Échec connexion : ${e}`);
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
  }
  $("terminal-empty").style.display = "none";
  const cur = state.sessions.get(id);
  sftpSyncButton();
  if (sftp.open && cur) sftpOpenAt(cur, cur.sftpPath);
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
    const first = state.sessions.keys().next();
    if (first.done) {
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
      focusSession(first.value);
    }
  }
}

// Écoute du flux PTY côté Rust → xterm
type PtyPayload = { id: number; data: string };
listenPty();
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
  try {
    state.hosts = await invoke<Host[]>("list_hosts");
  } catch (e) {
    console.warn("Config SSH illisible :", e);
  }
  rdpHostsList = await invoke<RdpHostT[]>("rdp_hosts").catch(() => []);
  state.folders = await invoke<string[]>("folders_list").catch(() => []);
  renderHosts();
  refreshEmptyHint();
}

// Search sidebar
const searchEl = $("search") as HTMLInputElement;
searchEl.addEventListener("input", () => { state.filter = searchEl.value; renderHosts(); });

// Palette
const paletteEl = $("palette") as HTMLDivElement;
const paletteInput = $("palette-input") as HTMLInputElement;
function paletteOpen() {
  paletteEl.classList.add("open");
  paletteInput.value = "";
  renderPalette();
  paletteInput.focus();
}
function paletteClose() { paletteEl.classList.remove("open"); }
function renderPalette() {
  const q = paletteInput.value.toLowerCase();
  const res = $("palette-results");
  res.innerHTML = "";
  const matches = filterHosts(state.hosts, q);
  if (matches.length === 0) {
    res.innerHTML = `<div class="empty">Aucun hôte pour « ${stripHtml(q)} »</div>`;
    return;
  }
  for (const h of matches) {
    const item = document.createElement("div");
    item.className = "item";
    item.innerHTML = `<span class="pico">${ic("terminal")}</span><span class="name"></span><span class="sub"></span>`;
    item.querySelector(".name")!.textContent = h.alias;
    item.querySelector(".sub")!.textContent = `${h.user ?? "?"}@${h.hostname ?? h.alias}`;
    item.addEventListener("click", () => { paletteClose(); openSession(h); });
    res.appendChild(item);
  }
}
paletteInput.addEventListener("input", renderPalette);
paletteInput.addEventListener("keydown", (e) => { if (e.key === "Escape") paletteClose(); });
paletteEl.addEventListener("click", (e) => { if (e.target === paletteEl) paletteClose(); });
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
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
    for (const e of sorted) {
      const el = document.createElement("div");
      el.className = "sftp-entry" + (e.is_dir ? " dir" : "");
      el.innerHTML = `<span class="ic"></span><span class="nm"></span><span class="sz"></span>`;
      el.querySelector(".ic")!.innerHTML = ic(fileIconName(e.name, e.is_dir));
      el.querySelector(".nm")!.textContent = e.name;
      el.querySelector(".sz")!.textContent = e.is_dir ? shortDate(e.modified) : humanSize(e.size);
      el.title = e.is_dir
        ? `${e.name} — modifié ${shortDate(e.modified) || "?"} — double-clic : ouvrir`
        : `${e.name} — ${humanSize(e.size)}, modifié ${shortDate(e.modified) || "?"} — double-clic : télécharger`;
      // Simple clic : selection. Double clic : ouvrir (dossier) / telecharger.
      el.addEventListener("click", () => {
        for (const n of list.querySelectorAll(".sftp-entry.sel")) n.classList.remove("sel");
        el.classList.add("sel");
      });
      el.addEventListener("dblclick", () => {
        if (e.is_dir) sftpNavigate(remoteJoin(path, e.name));
        else sftpDownload(remoteJoin(path, e.name), e.name);
      });
      el.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        sftpOpenMenu(e, path, ev as MouseEvent);
      });
      list.appendChild(el);
    }
    sftpStatus(`${entries.length} élément${entries.length > 1 ? "s" : ""}`);
  } catch (e) {
    list.innerHTML = "";
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

function sftpRefresh() {
  const s = sftpSession();
  if (s) sftpNavigate(s.sftpPath || ".");
}

function sftpToggle(force?: boolean) {
  const s = sftpSession();
  if (!s) return;
  sftp.open = force ?? !sftp.open;
  $("sftp-panel").classList.toggle("open", sftp.open);
  $("sftp-toggle").classList.toggle("active", sftp.open);
  if (sftp.open) sftpOpenAt(s, s.sftpPath);
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
  if (start) { sftpNavigate(start); return; }
  try {
    const home = await invoke<string>("sftp_realpath", { id: s.id, path: "." });
    if (sftpSession() === s) sftpNavigate(home || ".");
  } catch {
    sftpNavigate(".");
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
  if (sftpSession() === s) sftpNavigate(dir);
}

async function sftpPickAndUpload() {
  if (!sftpSession()) return;
  let picked: string[] | string | null = null;
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
    sftpNavigate(dir);
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
    sftpNavigate(dir);
  } catch (e) {
    sftpStatus(`⚠️ ${e}`, "err");
  }
}

async function sftpDelete(entry: SftpEntry, dir: string) {
  const s = sftpSession();
  if (!s) return;
  const what = entry.is_dir ? `le dossier « ${entry.name} » (doit être vide)` : `« ${entry.name} »`;
  if (!confirm(`Supprimer ${what} sur le serveur ?\n\nCette action est définitive.`)) return;
  try {
    await invoke("sftp_remove", { id: s.id, path: remoteJoin(dir, entry.name), isDir: entry.is_dir });
    sftpNavigate(dir);
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
  m.style.left = `${Math.min(e.clientX, window.innerWidth - 220)}px`;
  m.style.top = `${Math.min(e.clientY, window.innerHeight - 240)}px`;
  m.classList.add("open");
}
function sftpHideMenu() { $("sftp-context").classList.remove("open"); }
window.addEventListener("click", sftpHideMenu);
window.addEventListener("blur", sftpHideMenu);

$("sftp-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const ctx = sftp.ctx;
  sftpHideMenu();
  if (!act || !ctx) return;
  const s = sftpSession();
  if (!s) return;
  const { entry, path } = ctx;
  const full = entry ? remoteJoin(path, entry.name) : path;
  if (act === "download" && entry && !entry.is_dir) sftpDownload(full, entry.name);
  else if (act === "cd") {
    const target = entry?.is_dir ? full : path;
    invoke("pty_write", { id: s.id, data: `cd ${shellQuote(target)}\r` }).catch(() => {});
    s.term.focus();
  } else if (act === "copy") {
    navigator.clipboard.writeText(full).then(() => sftpStatus(`Chemin copié : ${full}`, "ok"), () => {});
  } else if (act === "rename" && entry) sftpRename(entry, path);
  else if (act === "mkdir") sftpMkdir(path);
  else if (act === "delete" && entry) sftpDelete(entry, path);
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
  if (s && s.sftpPath !== "/") sftpNavigate(parentDir(s.sftpPath || "."));
});
$("sftp-up-btn").addEventListener("click", sftpPickAndUpload);
$("sftp-mkdir-btn").addEventListener("click", () => {
  const s = sftpSession();
  if (s) sftpMkdir(s.sftpPath || ".");
});
$("sftp-path").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    const v = ($("sftp-path") as HTMLInputElement).value.trim();
    if (v) sftpNavigate(v);
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
      sftpUploadPaths(ev.payload.paths);
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
  if (e.key === "Escape" && $("ask-modal").classList.contains("open")) askClose(null);
});

hydrateIcons();
$("theme-toggle").addEventListener("click", cycleTheme);
applyTheme();
setupWindowControls();

loadHosts();
// Prechargement : au moment du clic, la police est deja prete.
ensureFontLoaded();

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
    if (($("m-rdp-save") as HTMLInputElement).checked) {
      try {
        await invoke("rdp_host_save", {
          id: null, name: ($("m-rdp-name") as HTMLInputElement).value.trim(),
          host: addr, port: rport, user, width: 0, height: 0,
        });
        if (($("m-rdp-remember") as HTMLInputElement).checked && password) {
          await invoke("rdp_password_save", { host: addr, port: rport, user, password }).catch(() => {});
        }
        await loadHosts();
      } catch (e) { manualError().textContent = String(e); manualError().hidden = false; return; }
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
      if (!alias) throw "Donne un nom à l'hôte pour l'enregistrer.";
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
    manualError().textContent = String(e);
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
  let keys: KeyEntry[] = [];
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
  if (e.key === "Escape" && passModal().classList.contains("open")) passClose(null);
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

function closeAllContextMenus() {
  for (const id of ["host-context", "rdp-context", "folder-context"]) $(id).classList.remove("open");
}
function openHostMenu(h: Host, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("host-context");
  m.dataset.alias = h.alias;
  m.style.left = `${e.clientX}px`;
  m.style.top = `${e.clientY}px`;
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
    openSession(h);
  } else if (act === "edit") {
    await openEditHost(alias);
  } else if (act === "move") {
    openMoveModal("ssh", alias);
  } else if (act === "tunnels") {
    await tunnelsOpen(alias);
  } else if (act === "delete") {
    const ok = confirm(
      `Supprimer l'hôte « ${alias} » de ~/.ssh/config ?\n\n` +
        `Son mot de passe mémorisé sera aussi oublié. Cette action est définitive.`,
    );
    if (!ok) return;
    try {
      await invoke("host_delete", { alias });
      await loadHosts();
    } catch (err) {
      alert(`Suppression impossible : ${err}`);
    }
  } else if (act === "forget") {
    try {
      await invoke("password_forget", {
        addr: h.hostname ?? h.alias,
        port: h.port,
        user: h.user ?? null,
      });
    } catch {
      /* pas de mot de passe memorise : rien a faire */
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
    alert(`Impossible de charger l'hôte : ${e}`);
  }
}

function closeEditHost() { $("edit-modal").classList.remove("open"); }

$("e-cancel").addEventListener("click", closeEditHost);
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if ($("edit-modal").classList.contains("open")) closeEditHost();
  if ($("rdp-edit-modal").classList.contains("open")) closeEditRdp();
  if ($("move-modal").classList.contains("open")) closeMoveModal();
  if ($("tunnels-modal").classList.contains("open")) tunnelsClose();
  if (snippetsModal().classList.contains("open")) snippetsClose();
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
      alert(`Arrêt impossible : ${e}`);
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
  const ok = confirm(`Supprimer le tunnel « ${d.name || describeTunnel(d)} » ?` +
    (tunnels.status.get(d.id)?.alive ? "\n\nIl est actif : il sera coupé." : ""));
  if (!ok) return;
  try {
    await invoke("tunnel_def_delete", { id: d.id });
  } catch (e) {
    alert(`Suppression impossible : ${e}`);
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
tunnelsRefresh();
window.setInterval(() => {
  if (tunnels.defs.length === 0) return;
  if (!tunnelsModal().classList.contains("open")) tunnelsRefresh();
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
  if (!confirm(`Supprimer le snippet « ${sn.name} » ?`)) return;
  try {
    await invoke("snippet_delete", { id: sn.id });
    await snippetsRefresh();
  } catch (e) {
    alert(`Suppression impossible : ${e}`);
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
  const paintMax = async () => {
    maxBtn.innerHTML = ic((await win.isMaximized()) ? "winRestore" : "winMax");
  };
  await paintMax();
  $("win-min").addEventListener("click", () => win.minimize());
  maxBtn.addEventListener("click", async () => { await win.toggleMaximize(); await paintMax(); });
  $("win-close").addEventListener("click", () => win.close());
  // L'état maximisé change aussi via double-clic système / raccourci.
  win.onResized(() => { void paintMax(); });
  // Fenêtre en arrière-plan : geler les animations (CPU au repos ~0).
  win.onFocusChanged(({ payload: focused }) => {
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
    const ok = confirm(
      `Version ${update.version} disponible (actuelle : ${update.currentVersion}).\n\n` +
        `${update.body ?? ""}\n\nTélécharger et installer maintenant ?`,
    );
    if (!ok) return;
    await update.downloadAndInstall();
    if (confirm("Mise à jour installée. Redémarrer Avash maintenant ?")) await relaunch();
  } catch (e) {
    // Endpoint injoignable / pas encore configuré / hors ligne : on le dit
    // sans dramatiser.
    ver.textContent = prev;
    alert(`Vérification des mises à jour impossible : ${e}`);
  } finally {
    updateBusy = false;
  }
}
$("app-version").addEventListener("click", checkForUpdates);

// ---------- RDP (bureau distant, via le sidecar avash-rdp) ----------


type RdpTarget = { host: string; port: number | null; user: string; password: string; width?: number; height?: number };

type RdpHostT = { id: string; name: string; host: string; port: number; user: string; width: number; height: number; folder: string };
let rdpHostsList: RdpHostT[] = [];
const RDP_ACK = new Uint8Array([6]); // accusé de rendu (cadencement adaptatif)

// Presse-papiers poste -> bureau distant (CLIPRDR). On lit le presse-papiers
// local et on l'annonce à la session RDP active quand Avash reprend le focus
// (tu copies ailleurs, tu reviens, tu colles dans le distant). Message [8].
let lastClipText = "";
async function pushLocalClipboard(): Promise<void> {
  if (state.active === null || !rdpSessions.has(state.active)) return;
  let text = "";
  try {
    text = (await clipReadText()) ?? "";
  } catch {
    return; // pas de texte (image/fichier) ou accès refusé
  }
  if (!text || text === lastClipText) return;
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
window.addEventListener("focus", () => void pushLocalClipboard());
const rdpSessions = new Map<number, { canvas: HTMLCanvasElement; tab: HTMLElement; ws: WebSocket | null; ro?: ResizeObserver }>();

async function openRdp(t: RdpTarget) {
  const id = state.nextId++;
  // Onglet
  const tabs = $("tabs");
  tabs.querySelector(".no-session")?.remove();
  const tab = document.createElement("div");
  tab.className = "tab active";
  tab.innerHTML = `<span class="state connecting"></span><span class="label"></span><span class="close"></span>`;
  tab.querySelector(".label")!.textContent = `🖥 ${t.user}@${t.host}`;
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
  rdpSessions.set(id, { canvas, tab, ws: null });
  state.active = id;

  tab.addEventListener("click", () => focusRdp(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => { e.stopPropagation(); closeRdp(id); });

  // Souris/clavier → sidecar via le WebSocket (binaire). Ignore si non prêt.
  const send = (bytes: number[]) => {
    const s = rdpSessions.get(id);
    if (s?.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array(bytes));
  };
  const pos = (e: MouseEvent): [number, number] => {
    // Le canvas est affiche en object-fit:contain : l'image est mise a l'echelle
    // pour tenir dans l'element en gardant son ratio, donc letterboxee (bandes).
    // On retrouve le rectangle reellement peint pour mapper le clic aux pixels RDP.
    const r = canvas.getBoundingClientRect();
    const scale = Math.min(r.width / rdpW, r.height / rdpH);
    const dispW = rdpW * scale;
    const dispH = rdpH * scale;
    const offX = (r.width - dispW) / 2;
    const offY = (r.height - dispH) / 2;
    const x = Math.max(0, Math.min(rdpW - 1, Math.round(((e.clientX - r.left - offX) / dispW) * rdpW)));
    const y = Math.max(0, Math.min(rdpH - 1, Math.round(((e.clientY - r.top - offY) / dispH) * rdpH)));
    return [x, y];
  };
  const le16 = (n: number) => [n & 0xff, (n >> 8) & 0xff];
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
    e.preventDefault(); const sc = rdpScancode(e.code); if (sc) send([4, ...le16(sc), 1]);
  });
  canvas.addEventListener("keyup", (e) => { e.preventDefault(); const sc = rdpScancode(e.code); if (sc) send([4, ...le16(sc), 0]); });

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

  // Bureau reçu via WebSocket local BINAIRE (ArrayBuffer natif : ni base64 ni
  // JSON — débit maximal, même en 3440×1440).
  //   [1] CONNECTED w,h · [2] FRAME x,y,w,h + RGBA · [3] ERROR utf8
  try {
    const conn = await invoke<{ port: number; token: string }>("rdp_open", {
      id, host: t.host, port: t.port, user: t.user, password: t.password,
      width: rdpW, height: rdpH,
    });
    const ws = new WebSocket(`ws://127.0.0.1:${conn.port}`);
    ws.binaryType = "arraybuffer";
    rdpSessions.get(id)!.ws = ws;
    ws.onopen = () => {
      ws.send(new TextEncoder().encode(conn.token));
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
        // Renégociation terminée : si la fenêtre a encore bougé entre-temps, on
        // applique la taille finale (une seule fois, évite les cascades).
        resizeInFlight = false;
        window.clearTimeout(resizeGuard);
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(sendResize, 120);
      } else if (kind === 8) {
        // Le bureau distant a copié du texte -> presse-papiers du poste.
        const text = new TextDecoder().decode(new Uint8Array(buf, 1));
        lastClipText = text; // ne pas le renvoyer aussitôt au distant
        clipWriteText(text).catch(() => {});
      } else if (kind === 3) {
        tab.querySelector(".state")!.className = "state closed";
        alert(`RDP : ${new TextDecoder().decode(new Uint8Array(buf, 1))}`);
      }
    };
    ws.onclose = () => {
      const st = tab.querySelector(".state");
      if (st) st.className = "state closed";
      tab.classList.add("dead");
    };
    ws.onerror = () => { /* onclose suivra */ };
  } catch (e) {
    tab.querySelector(".state")!.className = "state closed";
    alert(`Connexion RDP impossible : ${e}`);
  }
  focusRdp(id);
}

function focusRdp(id: number) {
  state.active = id;
  for (const [sid, s] of rdpSessions) {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    (s.canvas.parentElement as HTMLElement).style.display = active ? "flex" : "none";
    if (active) {
      s.canvas.focus();
      // Un canvas caché peut avoir perdu son contenu (backing-store WebKitGTK) :
      // on demande au sidecar de renvoyer l'image entière. Message [9].
      if (s.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array([9]));
    }
  }
  // Masquer les terminaux PTY.
  state.sessions.forEach((s) => { (s.term.element?.parentElement as HTMLElement).style.display = "none"; s.tab.classList.remove("active"); });
  $("terminal-empty").style.display = "none";
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
    state.active = null;
    if (state.sessions.size === 0 && rdpSessions.size === 0) $("terminal-empty").style.display = "flex";
  }
}

/** Connexion à un bureau RDP enregistré (mot de passe du trousseau, sinon demandé). */
async function connectRdpSaved(h: RdpHostT) {
  let pw = await invoke<string | null>("rdp_password_load", { host: h.host, port: h.port, user: h.user }).catch(() => null);
  if (!pw) {
    const rep = await askPassword(`${h.user}@${h.host}:${h.port}`);
    if (!rep) return;
    pw = rep.password;
    if (rep.remember && pw) {
      await invoke("rdp_password_save", { host: h.host, port: h.port, user: h.user, password: pw }).catch(() => {});
    }
  }
  await openRdp({ host: h.host, port: h.port, user: h.user, password: pw ?? "" });
}

function openRdpMenu(h: RdpHostT, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("rdp-context");
  m.dataset.id = h.id;
  m.style.left = `${Math.min(e.clientX, window.innerWidth - 220)}px`;
  m.style.top = `${Math.min(e.clientY, window.innerHeight - 160)}px`;
  m.classList.add("open");
}
window.addEventListener("click", () => $("rdp-context").classList.remove("open"));
$("rdp-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const id = $("rdp-context").dataset.id;
  $("rdp-context").classList.remove("open");
  const h = rdpHostsList.find((x) => x.id === id);
  if (!act || !h) return;
  if (act === "connect") connectRdpSaved(h);
  else if (act === "edit") openEditRdp(h);
  else if (act === "move") openMoveModal("rdp", h.id);
  else if (act === "forget") {
    await invoke("rdp_password_forget", { host: h.host, port: h.port, user: h.user }).catch(() => {});
  } else if (act === "delete") {
    if (!confirm(`Supprimer le bureau RDP « ${h.name} » ?\n\nSon mot de passe mémorisé sera aussi oublié.`)) return;
    await invoke("rdp_host_delete", { id: h.id }).catch((err) => alert(`Suppression impossible : ${err}`));
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
      const old = await invoke<string | null>("rdp_password_load", { host: oldHost, port: oldPort, user: oldUser }).catch(() => null);
      if (old) {
        await invoke("rdp_password_save", { host, port, user, password: old }).catch(() => {});
        await invoke("rdp_password_forget", { host: oldHost, port: oldPort, user: oldUser }).catch(() => {});
      }
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
function rdpScancode(code: string): number | null {
  const map: Record<string, number> = {
    Escape: 0x01, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05, Digit5: 0x06,
    Digit6: 0x07, Digit7: 0x08, Digit8: 0x09, Digit9: 0x0a, Digit0: 0x0b, Minus: 0x0c, Equal: 0x0d,
    Backspace: 0x0e, Tab: 0x0f, KeyQ: 0x10, KeyW: 0x11, KeyE: 0x12, KeyR: 0x13, KeyT: 0x14,
    KeyY: 0x15, KeyU: 0x16, KeyI: 0x17, KeyO: 0x18, KeyP: 0x19, BracketLeft: 0x1a, BracketRight: 0x1b,
    Enter: 0x1c, ControlLeft: 0x1d, KeyA: 0x1e, KeyS: 0x1f, KeyD: 0x20, KeyF: 0x21, KeyG: 0x22,
    KeyH: 0x23, KeyJ: 0x24, KeyK: 0x25, KeyL: 0x26, Semicolon: 0x27, Quote: 0x28, Backquote: 0x29,
    ShiftLeft: 0x2a, Backslash: 0x2b, KeyZ: 0x2c, KeyX: 0x2d, KeyC: 0x2e, KeyV: 0x2f, KeyB: 0x30,
    KeyN: 0x31, KeyM: 0x32, Comma: 0x33, Period: 0x34, Slash: 0x35, ShiftRight: 0x36,
    AltLeft: 0x38, Space: 0x39, CapsLock: 0x3a,
  };
  return map[code] ?? null;
}

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
    alert(`Cr\u00e9ation impossible : ${e}`);
  }
}

$("new-folder-btn").addEventListener("click", () => void createFolder(""));

function openFolderMenu(path: string, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("folder-context");
  m.dataset.path = path;
  m.style.left = `${Math.min(e.clientX, window.innerWidth - 220)}px`;
  m.style.top = `${Math.min(e.clientY, window.innerHeight - 160)}px`;
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
      alert(`Renommage impossible : ${ex}`);
    }
  } else if (act === "delete") {
    if (!confirm(`Supprimer le dossier \u00ab ${path} \u00bb ?\n\nLes h\u00f4tes qu'il contient (et ses sous-dossiers) reviennent \u00e0 la racine ; ils ne sont pas supprim\u00e9s.`)) return;
    try {
      await invoke("folder_delete", { path });
      await loadHosts();
    } catch (ex) {
      alert(`Suppression impossible : ${ex}`);
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
