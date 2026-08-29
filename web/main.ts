// Avash front v0.2 — terminal interactif réel : xterm.js ↔ PTY Rust (russh)

import "@xterm/xterm/css/xterm.css";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  humanSize, filterHosts, remoteJoin, parentDir, isPasswordRequired, stripHtml, hostInitials, hostHue,
  describeTunnel, tunnelFlag, tunnelTraffic, activeTunnelsByHost,
  type Host, type TunnelDef, type TunnelStatus, type TunnelKind,
} from "./filters";

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
    el.querySelector(".ini")!.textContent = hostInitials(h.alias);
    el.querySelector(".alias")!.textContent = h.alias;
    el.querySelector(".meta")!.textContent = target;
    // La pastille dit quelque chose de vrai : une session est ouverte ici.
    const dot = el.querySelector(".dot") as HTMLElement;
    dot.className = "dot " + hostSessionState(h.alias);
    el.title = "Double-clic : connexion — clic droit : options";
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

  const s: Session = { id, alias: label, term, fit, tab, search, closed: false, reconnect: null };
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
type SftpEntry = { name: string; is_dir: boolean; size: number; modified: number | null };

const sftp = {
  path: "" as string,
  open: false,
};


async function sftpNavigate(path: string) {
  const id = state.active;
  if (id === null) return;
  sftp.path = path;
  $("sftp-path").textContent = path;
  $("sftp-list").innerHTML = `<div class="sftp-status">Chargement…</div>`;
  try {
    const entries = await invoke<SftpEntry[]>("sftp_list", { id, path });
    const list = $("sftp-list");
    list.innerHTML = "";
    // Dossiers d'abord, tri alpha
    entries.sort((a, b) => (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0) || a.name.localeCompare(b.name));
    if (path !== "/") {
      const up = document.createElement("div");
      up.className = "sftp-entry dir";
      up.innerHTML = `<span>📁</span><span class="nm">..</span>`;
      up.addEventListener("click", () => {
        sftpNavigate(parentDir(path));
      });
      list.appendChild(up);
    }
    for (const e of entries) {
      const el = document.createElement("div");
      el.className = "sftp-entry" + (e.is_dir ? " dir" : "");
      el.innerHTML = `<span>${e.is_dir ? "📁" : "📄"}</span><span class="nm"></span><span class="sz"></span>`;
      el.querySelector(".nm")!.textContent = e.name;
      el.querySelector(".sz")!.textContent = e.is_dir ? "" : humanSize(e.size);
      if (e.is_dir) {
        el.addEventListener("click", () =>
          sftpNavigate(remoteJoin(path, e.name))
        );
      } else {
        el.title = "Clic : télécharger";
        el.addEventListener("click", async () => {
          const remote = remoteJoin(path, e.name);
          $("sftp-status").textContent = `⬇︎ ${e.name}…`;
          try {
            const res = await invoke<string>("sftp_download", { id, remote });
            $("sftp-status").textContent = `✅ ${res}`;
          } catch (err) {
            $("sftp-status").textContent = `⚠️ ${err}`;
          }
        });
      }
      list.appendChild(el);
    }
    $("sftp-status").textContent = `${entries.length} éléments`;
  } catch (e) {
    $("sftp-list").innerHTML = "";
    $("sftp-status").textContent = `⚠️ ${e}`;
  }
}

function sftpToggle() {
  const id = state.active;
  if (id === null) return;
  sftp.open = !sftp.open;
  $("sftp-panel").classList.toggle("open", sftp.open);
  if (sftp.open) {
    // "." = cwd du serveur au login (home en général)
    sftpNavigate(sftp.path.length > 0 ? sftp.path : ".");
  }
}

// Toggle SFTP : bouton rafraîchissement
$("sftp-refresh-btn").addEventListener("click", () => sftpNavigate(sftp.path || "."));
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
    e.preventDefault();
    sftpToggle();
  }
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
