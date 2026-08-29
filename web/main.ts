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
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  humanSize, filterHosts, remoteJoin, parentDir, isPasswordRequired, stripHtml, hostInitials, hostHue, osBadge,
  fileIcon, shortDate, shellQuote, validFileName, snippetPreview, snippetVars, renderSnippet, type SftpEntry, type Snippet,
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
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ⚠️ `fontFamily` n'appartient PAS au theme : c'est une option du Terminal.
// Place ici, il etait purement ignore et xterm.js retombait sur son defaut
// (courier-new), d'ou un rendu tres laid.
const THEME = {
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

function renderHosts() {
  const list = $("host-list");
  list.innerHTML = "";
  const shown = filterHosts(state.hosts, state.filter);
  for (const h of shown) {
    const el = document.createElement("div");
    const selected = state.active !== null && state.sessions.get(state.active)?.alias === h.alias;
    el.className = "host" + (selected ? " selected" : "");
    el.style.setProperty("--hue", hostHue(h.alias));
    const target = `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}`;
    el.innerHTML = `<span class="avatar"><span class="ini"></span><span class="dot"></span></span><span class="info">
      <div class="alias"></div><div class="meta"></div></span>`;
    const os = osByHost.get(h.alias);
    const ini = el.querySelector(".ini") as HTMLElement;
    if (os) {
      // Logo de la distribution (glyphe Nerd Font), couleur de marque.
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
    // La pastille dit quelque chose de vrai : une session est ouverte ici.
    const dot = el.querySelector(".dot") as HTMLElement;
    dot.className = "dot " + hostSessionState(h.alias);
    if (h.alias === state.pickedAlias) el.classList.add("picked");
    if (!os) el.title = "Double-clic : connexion — clic droit : options";
    // Simple clic : on surligne (retour visuel). Double clic : on connecte.
    el.addEventListener("click", () => {
      state.pickedAlias = h.alias;
      for (const n of list.querySelectorAll(".host.picked")) n.classList.remove("picked");
      el.classList.add("picked");
    });
    el.addEventListener("dblclick", () => openSession(h));
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      openHostMenu(h, e as MouseEvent);
    });
    list.appendChild(el);
  }
  $("host-count").textContent = String(shown.length);

  // Aucun hote declare : conseiller « double-clic sur un hote » n'aide
  // personne. On oriente vers la seule voie disponible.
  if (state.hosts.length === 0) {
    const empty = document.createElement("div");
    empty.className = "host-empty";
    empty.innerHTML =
      `<p>Aucun hôte dans <code>~/.ssh/config</code>.</p>` +
      `<p class="sub">Utilise <strong>Connexion directe</strong> ci-dessous, ` +
      `ou crée une clé puis installe-la sur un serveur.</p>`;
    list.appendChild(empty);
  } else if (shown.length === 0) {
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
}

/** Cree l'onglet et le terminal. La connexion elle-meme est faite par l'appelant. */
function newSessionShell(label: string) {
  const id = state.nextId++;
  const term = new Terminal({
    theme: THEME,
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
    item.innerHTML = `<span>😈</span><span class="name"></span><span class="sub"></span>`;
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
    entries.sort((a, b) => (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0) || a.name.localeCompare(b.name));
    if (path !== "/") {
      const up = document.createElement("div");
      up.className = "sftp-entry dir up";
      up.innerHTML = `<span class="ic">↰</span><span class="nm">..</span><span class="sz"></span>`;
      up.addEventListener("dblclick", () => sftpNavigate(parentDir(path)));
      list.appendChild(up);
    }
    for (const e of entries) {
      const el = document.createElement("div");
      el.className = "sftp-entry" + (e.is_dir ? " dir" : "");
      el.innerHTML = `<span class="ic"></span><span class="nm"></span><span class="sz"></span>`;
      el.querySelector(".ic")!.textContent = fileIcon(e.name, e.is_dir);
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
  if (!s || sftp.busy) return;
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
$("ask-modal").addEventListener("click", (e) => { if (e.target === $("ask-modal")) askClose(null); });
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("ask-modal").classList.contains("open")) askClose(null);
});

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
  manualModal().classList.add("open");
  ($("m-addr") as HTMLInputElement).focus();
}

function manualClose() {
  manualModal().classList.remove("open");
  ($("manual-form") as HTMLFormElement).reset();
  manualSyncAuthRows();
  manualSyncSaveRow();
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
// Fermeture au clic hors du cadre, et a Echap.
manualModal().addEventListener("click", (e) => {
  if (e.target === manualModal()) manualClose();
});
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
keysModal().addEventListener("click", (e) => {
  if (e.target === keysModal()) keysClose();
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && keysModal().classList.contains("open")) keysClose();
});

// ---------- Raccourcis d'onglets ----------

/** Liste ordonnee des identifiants de session, pour naviguer par position. */
function sessionIds(): number[] {
  return [...state.sessions.keys()];
}

function cycleSession(step: number) {
  const ids = sessionIds();
  if (ids.length < 2) return;
  const i = ids.indexOf(state.active ?? ids[0]);
  focusSession(ids[(i + step + ids.length) % ids.length]);
}

window.addEventListener("keydown", (e) => {
  // Ne pas capturer pendant qu'un formulaire est ouvert : l'utilisateur
  // y tape, Ctrl+W fermerait un onglet sous ses doigts.
  if (document.querySelector(".modal-backdrop.open")) return;
  const mod = e.ctrlKey || e.metaKey;
  if (!mod) return;

  if (e.key.toLowerCase() === "w" && state.active !== null) {
    e.preventDefault();
    closeSession(state.active);
    return;
  }
  if (e.key === "Tab") {
    e.preventDefault();
    cycleSession(e.shiftKey ? -1 : 1);
    return;
  }
  // Ctrl+1..9 : acces direct a un onglet par sa position.
  if (/^[1-9]$/.test(e.key)) {
    const ids = sessionIds();
    const idx = Number(e.key) - 1;
    if (idx < ids.length) {
      e.preventDefault();
      focusSession(ids[idx]);
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
passModal().addEventListener("click", (e) => {
  if (e.target === passModal()) passClose(null);
});
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

function openHostMenu(h: Host, e: MouseEvent) {
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
    $("edit-modal").classList.add("open");
    setTimeout(() => ($("e-alias") as HTMLInputElement).focus(), 30);
  } catch (e) {
    alert(`Impossible de charger l'hôte : ${e}`);
  }
}

function closeEditHost() { $("edit-modal").classList.remove("open"); }

$("e-cancel").addEventListener("click", closeEditHost);
$("edit-modal").addEventListener("click", (e) => {
  if (e.target === $("edit-modal")) closeEditHost();
});
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if ($("edit-modal").classList.contains("open")) closeEditHost();
  if ($("tunnels-modal").classList.contains("open")) tunnelsClose();
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
        <button class="tbtn" data-act="edit" title="Modifier">✎</button>
        <button class="tbtn danger" data-act="delete" title="Supprimer">🗑</button>
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
      toggle.textContent = "■ Arrêter";
      toggle.className = "tbtn stop";
    } else {
      toggle.textContent = running ? "↻ Relancer" : "▶ Démarrer";
      toggle.className = "tbtn go";
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
tunnelsModal().addEventListener("click", (e) => {
  if (e.target === tunnelsModal()) tunnelsClose();
});
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

// Badges de la barre laterale : rafraichis a l'ouverture puis toutes les
// 5 s, pour refleter un tunnel tombe meme quand la modale est fermee.
tunnelsRefresh();
window.setInterval(() => {
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
        <button class="tbtn go" data-act="send" title="Envoyer">▶</button>
        <button class="tbtn" data-act="edit" title="Modifier">✎</button>
        <button class="tbtn danger" data-act="delete" title="Supprimer">🗑</button>
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
snippetsModal().addEventListener("click", (e) => { if (e.target === snippetsModal()) snippetsClose(); });

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
    const label = document.createElement("label");
    label.innerHTML = `<span></span><input data-var="${v}" spellcheck="false" />`;
    label.querySelector("span")!.textContent = v;
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
$("send-modal").addEventListener("click", (e) => { if (e.target === $("send-modal")) $("send-modal").classList.remove("open"); });
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
