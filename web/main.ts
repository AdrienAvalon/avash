// Avash front v0.2 — terminal interactif réel : xterm.js ↔ PTY Rust (russh)

import "@xterm/xterm/css/xterm.css";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ic, hydrateIcons } from "./icons";
import { partageClipboard, setPartageClipboard } from "./prefs";
import { filterHosts, isPasswordRequired, isHostKeyChanged, stripHtml, hostInitials, hostHue, osBadge, type Host, type OsInfo, buildFolderTree, folderNodeCount, type FolderNode } from "./filters";
import { $, type RdpHostT, type Session, collapsedFolders, osByHost, rememberOs, saveCollapsed, state } from "./etat";
import { FONT_STACK, applyTheme, cycleTheme, ensureFontLoaded, hostSessionState, renderTagBar, terminalTheme } from "./theme";
import { MENUS_CONTEXTUELS, openHostMenu, ouvrirMenuAuClavier } from "./menu-hote";
import { type ManualTarget } from "./connexion-directe";
import { annoncerPartageClip, connectRdpSaved, marquerVisibilite, openRdpMenu, rdpSessions } from "./rdp";
import { askConfirm, askPassword, collerDansTerminal } from "./dialogues";
import { focusTab, orderedTabs } from "./raccourcis";
import { notify, notifyErreur } from "./notifications";
import { openFolderMenu } from "./dossiers";
import { openTermSearch, setFontSize } from "./terminal-outils";
import { setTitlebar, setupWindowControls } from "./titre";
import { sftp, sftpOpenAt, sftpSyncButton } from "./sftp";
import { tunnels } from "./tunnels";

// ---------- Arbre des hôtes (dossiers unifiés SSH + RDP) ----------

type TreeItem = { kind: "ssh"; ssh: Host } | { kind: "rdp"; rdp: RdpHostT };
type TreeNode = FolderNode<TreeItem>;

/** Construit l'arbre unifié à partir du registre de dossiers et des hôtes SSH+RDP.
 *  (Logique pure dans filters.ts, testée ; ici on ne fait que l'alimenter.) */
function buildTree(): TreeNode {
  return buildFolderTree<TreeItem>(state.folders, [
    ...state.hosts.map((h) => ({ folder: h.folder ?? "", item: { kind: "ssh", ssh: h } as TreeItem })),
    ...state.rdpHosts.map((h) => ({ folder: h.folder ?? "", item: { kind: "rdp", rdp: h } as TreeItem })),
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
export async function moveHostTo(kind: string, id: string, folder: string) {
  try {
    if (kind === "ssh") await invoke("host_set_folder", { alias: id, folder });
    else await invoke("rdp_host_set_folder", { id, folder });
    await loadHosts();
  } catch (e) {
    notifyErreur(`Déplacement impossible : ${e}`);
  }
}

/** Rend un élément « cible de dépôt » pour ranger un hôte dans `folder`. */
export function setupFolderDrop(el: HTMLElement, folder: string, hover = true) {
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

export function renderHosts() {
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
  const rdpShown = state.tagFilter !== null ? [] : state.rdpHosts.filter(
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

  if (state.hosts.length === 0 && state.rdpHosts.length === 0) {
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
  if (state.tagFilter !== null && state.rdpHosts.length > 0) {
    const note = document.createElement("div");
    note.className = "host-empty";
    note.innerHTML =
      `<p class="sub">${state.rdpHosts.length} bureau${state.rdpHosts.length > 1 ? "x" : ""} RDP ` +
      `masqué${state.rdpHosts.length > 1 ? "s" : ""} : les bureaux ne portent pas de tag.</p>`;
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
    fontSize: state.terminalFontSize,
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
        (t) => void collerDansTerminal(term, t),
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
      setFontSize(state.terminalFontSize + 1);
      return false;
    }
    if (e.ctrlKey && (e.code === "Minus" || e.code === "NumpadSubtract")) {
      setFontSize(state.terminalFontSize - 1);
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
export async function openSession(h: Host) {
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
/// `alias` : le nom sous lequel l'hôte vient d'être enregistré, s'il l'a été.
///
/// Sans lui, l'onglet s'intitulait `utilisateur@adresse` alors que l'hôte
/// portait un alias, et la session n'était rattachée à aucune ligne de la barre
/// latérale — voyant éteint, menu qui ne la reconnaît pas. Il fallait fermer
/// l'onglet et se reconnecter depuis la liste pour retrouver le bon nom.
export async function openManualSession(t: ManualTarget, alias?: string) {
  await ensureFontLoaded();
  const { id, term, session } = newSessionShell(alias?.trim() || `${t.user}@${t.addr}`);
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
  // Le cœur renvoie toujours « utilisateur@adresse ». L'onglet, lui, a déjà été
  // nommé par `newSessionShell` — avec l'alias sous lequel l'hôte vient d'être
  // enregistré, le cas échéant. L'écraser ici rendait le titre faux, et il le
  // restait jusqu'à ce qu'on ferme l'onglet et rouvre depuis la barre latérale.
  if (!s.alias) {
    s.alias = label;
    s.tab.querySelector(".label")!.textContent = label;
  }
  setSessionState(id, "live");
}

function focusTerminal(s: Session) {
  s.term.focus();
  requestAnimationFrame(() => s.fit.fit());
}

export function focusSession(id: number) {
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

export function closeSession(id: number) {
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

export async function loadHosts() {
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
  state.rdpHosts = bureaux;
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
    ...state.rdpHosts
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

// ---------- Amorçage ----------
//
// En dernier, une fois tout branché : icônes, thème, contrôles de fenêtre, puis
// la liste des hôtes et la police du terminal.

hydrateIcons();
$("theme-toggle").addEventListener("click", cycleTheme);
applyTheme();
void setupWindowControls();

void loadHosts();
// Prechargement : au moment du clic, la police est deja prete.
void ensureFontLoaded();
