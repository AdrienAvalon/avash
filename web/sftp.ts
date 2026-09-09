// Panneau SFTP : liste, transferts, menu contextuel, glisser-déposer.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ic, fileIconName } from "./icons";
import { humanSize, remoteJoin, parentDir, sortSftpEntries, shortDate, shellQuote, validFileName, type SftpEntry } from "./filters";
import { $, type Session, state, ciblesDeCopie } from "./etat";
import { askConfirm, askText } from "./dialogues";
import { placerMenu, ouvrirMenuAuClavier } from "./menu-hote";
import { langue, t } from "./i18n";

// ===== SFTP =====

export const sftp = {
  open: false,
  /** Entree visee par le menu contextuel. */
  ctx: null as { entry: SftpEntry | null; path: string } | null,
};

// ===== File des transferts =====
//
// Plusieurs transferts à la fois (trois en parallèle, les autres attendent),
// chacun avec sa ligne : nom, progression, vitesse, bouton d'annulation.
// L'identifiant est choisi ici et suit le transfert jusque dans le cœur, qui
// le rappelle dans chaque événement de progression et le retrouve pour
// l'annuler.

type Transfert = {
  id: number;
  kind: "download" | "upload" | "copie";
  nom: string;
  fichier: string;
  fait: number;
  total: number;
  termines: number;
  nombre: number;
  etat: "attente" | "en-cours" | "fini" | "erreur" | "annule";
  vitesse: number;
  message: string;
  lancer: () => Promise<string>;
  /** Pour la vitesse : dernier point mesuré. */
  dernierT: number;
  dernierFait: number;
  /** Session d'origine, pour rafraîchir sa liste à la fin. */
  session: Session;
  /** Copie directe (scp menée par l'hôte source) : le cœur n'inscrit aucun
   *  drapeau et n'émet aucune progression pour elle. Trouvé par l'audit du
   *  7 septembre 2026 : sans ce repère la ligne offrait un bouton « Annuler »
   *  sans effet et restait bloquée à « 0 o ». */
  direct: boolean;
};

const PARALLELE = 3;
const file: Transfert[] = [];
let prochainTransfert = 1;

/** Ajoute un transfert à la file et lance ce qui peut l'être. */
function ajouterTransfert(kind: Transfert["kind"], nom: string, session: Session, lancer: (id: number) => Promise<string>, direct = false): void {
  const id = prochainTransfert++;
  file.push({
    id, kind, nom, fichier: "", fait: 0, total: 0, termines: 0, nombre: 1,
    etat: "attente", vitesse: 0, message: "", lancer: () => lancer(id),
    dernierT: 0, dernierFait: 0, session, direct,
  });
  rendreTransferts();
  planifier();
}

function planifier(): void {
  for (const x of file) {
    if (x.etat !== "attente") continue;
    if (file.filter((y) => y.etat === "en-cours").length >= PARALLELE) break;
    x.etat = "en-cours";
    x.dernierT = performance.now();
    void x.lancer().then(
      (message) => { x.etat = "fini"; x.message = message; x.fait = x.total || x.fait; terminer(x); },
      (err) => {
        const texte = String(err);
        x.etat = texte.includes("Transfert annulé") ? "annule" : "erreur";
        x.message = texte;
        terminer(x);
      },
    );
  }
  rendreTransferts();
}

function terminer(x: Transfert): void {
  rendreTransferts();
  // La liste du panneau reflète ce qui vient d'arriver ou de partir.
  if (sftpSession() === x.session && (x.kind === "upload" || x.kind === "copie") && x.etat === "fini") void sftpNavigate(x.session.sftpPath || ".");
  if (x.etat === "fini") {
    sftpStatus(`✅ ${x.nom} : ${x.message}`, "ok");
    window.setTimeout(() => { const i = file.indexOf(x); if (i >= 0 && x.etat === "fini") { file.splice(i, 1); rendreTransferts(); } }, 8000);
  } else if (x.etat === "annule") {
    sftpStatus(`${x.nom} : ${t("sftp-transfert-annule")}`, "");
  } else {
    sftpStatus(`⚠️ ${x.nom} : ${x.message}`, "err");
  }
  planifier();
}

/** Dessine la file ; les lignes finies s'effacent d'elles-mêmes, les erreurs
 *  et les annulations restent jusqu'à un clic. */
function rendreTransferts(): void {
  const zone = $("sftp-transferts");
  zone.innerHTML = "";
  for (const x of file) {
    const el = document.createElement("div");
    el.className = `sftp-transfert ${x.etat}`;
    el.setAttribute("role", "listitem");
    const fleche = x.kind === "download" ? "⬇︎" : x.kind === "upload" ? "⬆︎" : "⇄";
    const pct = x.total ? Math.round((x.fait / x.total) * 100) : 0;
    let det: string;
    if (x.etat === "en-cours" && x.direct && x.kind === "copie") {
      // Copie directe : aucun événement de progression (scp chez la source),
      // « 0 o » laissait croire à un transfert figé — on dit ce qui se passe.
      det = t("sftp-copie-directe-en-cours");
    } else if (x.etat === "en-cours") {
      const vit = x.vitesse > 0 ? ` · ${humanSize(Math.round(x.vitesse), langue())}/s` : "";
      det = x.nombre > 1
        ? `${t("sftp-elements-faits", { fait: humanSize(x.fait, langue()), total: humanSize(x.total, langue()), termines: x.termines, nombre: x.nombre })}${vit}${x.fichier ? ` · ${x.fichier}` : ""}`
        : `${humanSize(x.fait, langue())}${x.total ? ` / ${humanSize(x.total, langue())}` : ""}${vit}`;
    } else if (x.etat === "attente") det = "…";
    else if (x.etat === "fini") det = t("sftp-transfert-fini");
    else if (x.etat === "annule") det = t("sftp-transfert-annule");
    else det = x.message;
    el.innerHTML = `<span class="nm"></span><button type="button" class="btn-mini" hidden></button><span class="det"></span><span class="barre"><span></span></span>`;
    el.querySelector(".nm")!.textContent = `${fleche} ${x.nom}`;
    el.querySelector(".det")!.textContent = det;
    (el.querySelector(".barre > span") as HTMLElement).style.width = `${x.etat === "fini" ? 100 : pct}%`;
    const bouton = el.querySelector("button") as HTMLButtonElement;
    if (boutonAnnulerVisible(x.etat, x.kind, x.direct)) {
      bouton.hidden = false;
      bouton.textContent = t("sftp-annuler");
      bouton.title = t("sftp-annuler");
      bouton.addEventListener("click", () => annulerTransfert(x));
    } else if (x.etat !== "en-cours" && x.etat !== "attente") {
      // Une ligne terminée s'efface d'un clic.
      el.style.cursor = "pointer";
      el.addEventListener("click", () => { const i = file.indexOf(x); if (i >= 0) { file.splice(i, 1); rendreTransferts(); } });
    }
    // Sinon (copie directe en cours) : ni bouton — l'hôte source mène le scp,
    // rien ici ne peut l'interrompre — ni clic d'effacement (le scp tourne).
    zone.appendChild(el);
  }
}

/** Le bouton « Annuler » n'a de sens que si l'annulation peut aboutir.
 *
 *  Extrait pour le test (audit du 7 septembre 2026). Une copie directe *en
 *  cours* est menée par scp chez l'hôte source : le cœur ne lève aucun drapeau,
 *  sftp_annuler rend false et rien ne s'arrête — proposer le bouton mentait.
 *  Tant qu'elle *attend* son tour, en revanche, l'annulation est purement
 *  locale (le scp n'a pas démarré) et le bouton reste légitime. */
export function boutonAnnulerVisible(etat: Transfert["etat"], kind: Transfert["kind"], direct: boolean): boolean {
  if (etat === "attente") return true;
  if (etat === "en-cours") return !(direct && kind === "copie");
  return false;
}

export async function annulerTransfert(x: Transfert): Promise<void> {
  if (x.etat === "attente") {
    x.etat = "annule";
    x.message = t("sftp-transfert-annule");
    rendreTransferts();
    return;
  }
  // Trouvé par l'audit du 7 septembre 2026 : le booléen rendu par sftp_annuler
  // était jeté. Quand il vaut false — aucun transfert inscrit sous cet id : la
  // fenêtre entre le clic et inscrire() (ouverture du canal en attente du verrou
  // de session), ou une copie directe non interruptible — le clic restait sans
  // le moindre effet ni mot. On le dit désormais au lieu de laisser croire à une
  // annulation qui n'a pas eu lieu.
  let ok: boolean;
  try { ok = await invoke<boolean>("sftp_annuler", { transfert: x.id }); }
  catch { ok = false; }
  if (!ok) sftpStatus(t("sftp-annulation-impossible", { nom: x.nom }), "err");
}

/** Un événement de progression du cœur, rapporté à sa ligne. */
function progressionTransfert(p: { transfert: number; fichier: string; done: number; total: number; termines: number; nombre: number }): void {
  const x = file.find((y) => y.id === p.transfert);
  if (!x) return;
  const maintenant = performance.now();
  const dt = (maintenant - x.dernierT) / 1000;
  if (dt >= 0.5) {
    const instant = (p.done - x.dernierFait) / dt;
    // Lissée : une moyenne mobile, pour que le chiffre ne tremble pas.
    x.vitesse = x.vitesse > 0 ? x.vitesse * 0.6 + instant * 0.4 : instant;
    x.dernierT = maintenant;
    x.dernierFait = p.done;
  }
  x.fichier = p.nombre > 1 ? p.fichier : "";
  x.fait = p.done;
  x.total = p.total;
  x.termines = p.termines;
  x.nombre = p.nombre;
  rendreTransferts();
}

function sftpSession(): Session | null {
  if (state.active === null) return null;
  const s = state.sessions.get(state.active) ?? null;
  // Un port série n'a pas de système de fichiers à montrer.
  return s?.serie ? null : s;
}

function sftpStatus(msg: string, kind: "" | "ok" | "err" = "") {
  const el = $("sftp-status");
  el.textContent = msg;
  el.className = "sftp-status" + (kind ? ` ${kind}` : "");
}

async function sftpNavigate(path: string) {
  const s = sftpSession();
  if (!s) return;
  s.sftpPath = path;
  ($("sftp-path") as HTMLInputElement).value = path;
  const list = $("sftp-list");
  list.innerHTML = `<div class="sftp-status">${t("chargement")}</div>`;
  try {
    const entries = await invoke<SftpEntry[]>("sftp_list", { id: s.id, path });
    // La reponse peut arriver apres un changement d'onglet.
    if (sftpSession() !== s || s.sftpPath !== path) return;
    list.innerHTML = "";
    const sorted = sortSftpEntries(entries);
    if (path !== "/") {
      const up = document.createElement("div");
      up.className = "sftp-entry dir up";
      // Trouvé par l'audit du 7 septembre 2026 : les entrées n'étaient ni
      // focalisables ni annoncées, la liste restait hors d'atteinte au clavier.
      up.tabIndex = -1; // le tabindex glissant en désignera une seule à 0
      up.setAttribute("role", "button");
      up.setAttribute("aria-label", t("dossier-parent"));
      up.title = `.. — ${t("gestes-ligne")}`;
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
      // Trouvé par l'audit du 7 septembre 2026 : chaque entrée devient un bouton
      // focalisable (tabindex glissant plus bas) et annoncé par son nom, faute de
      // quoi le clavier n'atteignait ni les dossiers, ni le téléchargement.
      el.tabIndex = -1;
      el.setAttribute("role", "button");
      el.setAttribute("aria-label", e.name);
      el.firstChild!.appendChild(icone(fileIconName(e.name, e.is_dir)));
      el.querySelector(".nm")!.textContent = e.name;
      el.querySelector(".sz")!.textContent = e.is_dir ? shortDate(e.modified, new Date(), langue()) : humanSize(e.size, langue());
      // Les gestes clavier sont rappelés dans l'infobulle, seul endroit où les découvrir.
      el.title = (e.is_dir
        ? t("sftp-titre-dossier", { nom: e.name, date: shortDate(e.modified, new Date(), langue()) || "?" })
        : t("sftp-titre-fichier", { nom: e.name, taille: humanSize(e.size, langue()), date: shortDate(e.modified, new Date(), langue()) || "?" }))
        + ` — ${t("gestes-ligne")}`;
      lot.appendChild(el);
    });
    list.appendChild(lot);

    // Délégation : quatre écouteurs pour toute la liste, au lieu d'autant par
    // entrée. `sftpDelegue` est réarmé à chaque navigation avec le lot courant.
    sftpDelegue(list, sorted, path);
    // Tabindex glissant : une seule entrée reçoit l'arrêt de tabulation, on
    // entre dans la liste d'un Tab puis on s'y déplace aux flèches. Poser
    // tabindex=0 partout aurait exigé des milliers de Tab sur /usr/bin.
    const premier = list.querySelector<HTMLElement>(".sftp-entry");
    if (premier) premier.tabIndex = 0;
    sftpStatus(t(entries.length > 1 ? "sftp-elements" : "sftp-element", { n: entries.length }));
  } catch (e) {
    // Trouvé par l'audit du 7 septembre 2026 : même garde anti-course que le
    // succès (plus haut). Un listage périmé (autre onglet, dossier précédent,
    // ou permission refusée) qui rejette après qu'un listage plus récent a été
    // rendu vidait la liste courante et affichait une erreur étrangère au
    // dossier montré, réparable seulement par Rafraîchir.
    if (sftpSession() !== s || s.sftpPath !== path) return;
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
  // Le simple clic sélectionne, y compris « .. » : c'est un repère visuel.
  list.addEventListener("click", (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>(".sftp-entry.up");
    if (!el) return;
    for (const n of list.querySelectorAll(".sftp-entry.sel")) n.classList.remove("sel");
    el.classList.add("sel");
  });
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
    else sftpDownload(remoteJoin(sftpLot.path, e.name), e.name);
  });
  list.addEventListener("contextmenu", (ev) => {
    const cible = viser(ev);
    if (!cible) return;
    ev.preventDefault();
    sftpOpenMenu(cible.e, sftpLot.path, ev as MouseEvent);
  });

  // Navigation au clavier. Trouvé par l'audit du 7 septembre 2026 : les entrées
  // n'avaient que des gestes souris, on ne pouvait ni entrer dans un dossier, ni
  // télécharger, ni ouvrir le menu sans souris (la barre latérale, elle, l'avait
  // déjà — voir rendreAtteignableAuClavier dans main.ts). Un seul écouteur
  // délégué plutôt qu'un par ligne : /usr/bin (~4000 entrées) en poserait autant.
  //
  // Le focus surligne l'entrée (`.hl`, distincte de la sélection `.sel`) et lui
  // donne l'unique arrêt de tabulation.
  list.addEventListener("focusin", (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>(".sftp-entry");
    if (!el) return;
    for (const n of list.querySelectorAll(".sftp-entry.hl")) n.classList.remove("hl");
    el.classList.add("hl");
    for (const n of list.querySelectorAll<HTMLElement>(".sftp-entry")) n.tabIndex = n === el ? 0 : -1;
  });
  // Enter = double-clic (naviguer ou télécharger) ; Maj+F10 et la touche Menu
  // ouvrent le menu au bord de l'entrée ; les flèches, Origine et Fin déplacent.
  const activer = (el: HTMLElement): void => {
    if (el.classList.contains("up")) { void sftpNavigate(parentDir(sftpLot.path)); return; }
    const e = sftpLot.entries[Number(el.dataset.i)];
    if (!e) return;
    if (e.is_dir) void sftpNavigate(remoteJoin(sftpLot.path, e.name));
    else sftpDownload(remoteJoin(sftpLot.path, e.name), e.name);
  };
  const ouvrirMenu = (el: HTMLElement): void => {
    const r = el.getBoundingClientRect();
    // Au bord de l'entrée ; `placerMenu` recadre si le menu dépasse la fenêtre.
    const pos = { clientX: r.left + 16, clientY: r.bottom - 4 } as MouseEvent;
    if (el.classList.contains("up")) sftpOpenMenu(null, sftpLot.path, pos);
    else {
      const e = sftpLot.entries[Number(el.dataset.i)];
      if (!e) return;
      sftpOpenMenu(e, sftpLot.path, pos);
    }
    ouvrirMenuAuClavier($("sftp-context"), el);
  };
  list.addEventListener("keydown", (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>(".sftp-entry");
    if (!el) return;
    if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); activer(el); return; }
    if (ev.key === "ContextMenu" || (ev.key === "F10" && ev.shiftKey)) { ev.preventDefault(); ouvrirMenu(el); return; }
    const entrees = [...list.querySelectorAll<HTMLElement>(".sftp-entry")];
    const i = entrees.indexOf(el);
    const vise =
      ev.key === "ArrowDown" ? i + 1
      : ev.key === "ArrowUp" ? i - 1
      : ev.key === "Home" ? 0
      : ev.key === "End" ? entrees.length - 1
      : null;
    if (vise === null) return;
    ev.preventDefault();
    entrees[Math.max(0, Math.min(vise, entrees.length - 1))]?.focus();
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
export function sftpSyncButton() {
  const has = sftpSession() !== null;
  ($("sftp-toggle") as HTMLButtonElement).disabled = !has;
  $("sftp-toggle").classList.toggle("active", has && sftp.open);
}

/** Reflète l'état COMPLET du panneau SFTP selon l'onglet courant : le bouton
 *  (via `sftpSyncButton`) et la visibilité du panneau lui-même.
 *
 *  Trouvé par l'audit du 7 septembre 2026 : `focusRdp` (et `closeRdp` quand il
 *  ne reste aucun onglet) ne resynchronisaient rien. En passant d'un onglet SSH
 *  au panneau ouvert vers un bureau RDP, le bouton restait cliquable et « actif »
 *  alors qu'un clic ne faisait rien (un bureau distant n'a pas de système de
 *  fichiers, `sftpSession()===null`), et le panneau restait ouvert à côté du
 *  bureau, affichant les fichiers de la session SSH précédente.
 *
 *  On pose donc la classe « open » à partir de `has && sftp.open`, ce qui masque
 *  le panneau sur un onglet sans SFTP sans toucher à `sftp.open` : cette
 *  préférence de l'utilisateur reste vraie et le panneau reparaît tel quel au
 *  retour sur l'onglet SSH (sinon retirer la classe seule désynchroniserait
 *  l'état et il faudrait deux Ctrl+B pour le revoir). À appeler à chaque bascule
 *  ou fermeture d'onglet (`focusSession`, `focusRdp`, `closeSession`,
 *  `closeRdp`). */
export function sftpAppliquerVue() {
  sftpSyncButton();
  $("sftp-panel").classList.toggle("open", sftpSession() !== null && sftp.open);
}

/**
 * Ouvre le panneau sur un dossier de depart : on resout d'abord "." en
 * chemin absolu (certains serveurs refusent read_dir(".")), puis on liste.
 */
export async function sftpOpenAt(s: Session, path: string) {
  const start = path && path !== "." ? path : "";
  if (start) { void sftpNavigate(start); return; }
  try {
    const home = await invoke<string>("sftp_realpath", { id: s.id, path: "." });
    if (sftpSession() === s) void sftpNavigate(home || ".");
  } catch {
    // Trouvé par l'audit du 7 septembre 2026 : même garde que la branche de
    // succès. Un sftp_realpath lent qui échoue sur l'ancien onglet renvoyait le
    // panneau de l'onglet courant sur « . » et écrasait son sftpPath.
    if (sftpSession() === s) void sftpNavigate(".");
  }
}
// Le panneau prend sa place par une transition : le terminal ne recoit pas
// d'evenement resize et resterait coupe a droite. On l'ajuste a la fin.
$("sftp-panel").addEventListener("transitionend", (e) => {
  if (e.propertyName === "width") sftpSession()?.fit.fit();
});

/** Télécharge un fichier ou un dossier distant, dans la file. */
function sftpDownload(remote: string, name: string, isDir = false) {
  const s = sftpSession();
  if (!s) return;
  ajouterTransfert("download", name, s, (transfert) => invoke<string>("sftp_download", { id: s.id, transfert, remote, isDir }));
}

/** Envoie des fichiers ou dossiers locaux (chemins absolus) dans le dossier
 *  courant, chacun sur sa ligne de la file. */
function sftpUploadPaths(paths: string[]) {
  const s = sftpSession();
  if (!s || paths.length === 0) return;
  const dir = s.sftpPath || ".";
  for (const local of paths) {
    const name = local.split(/[\\/]/).pop() ?? local;
    ajouterTransfert("upload", name, s, (transfert) => invoke<string>("sftp_upload", { id: s.id, transfert, local, remoteDir: dir }));
  }
}

// ----- Copier vers un autre hôte -----

let copieVisee: { entry: SftpEntry; path: string } | null = null;

/** Ouvre la modale de copie vers un autre onglet SSH. */
function sftpOuvrirCopie(entry: SftpEntry, path: string): void {
  const s = sftpSession();
  if (!s) return;
  const autres = ciblesDeCopie(s, state.sessions);
  if (autres.length === 0) {
    sftpStatus(t("sftp-aucune-autre-session"), "err");
    return;
  }
  copieVisee = { entry, path };
  const select = $("sc-cible") as HTMLSelectElement;
  select.innerHTML = "";
  for (const x of autres) {
    const o = document.createElement("option");
    o.value = String(x.id);
    o.textContent = x.tab.querySelector(".label")?.textContent ?? `#${x.id}`;
    select.appendChild(o);
  }
  ($("sc-dossier") as HTMLInputElement).value = "";
  ($("sc-direct") as HTMLInputElement).checked = false;
  $("sc-error").hidden = true;
  $("sftp-copier-quoi").textContent = t("sftp-copier-quoi", { quoi: entry.name, source: s.tab.querySelector(".label")?.textContent ?? "" });
  $("sftp-copier-modal").classList.add("open");
  setTimeout(() => select.focus(), 30);
}

function sftpFermerCopie(): void {
  $("sftp-copier-modal").classList.remove("open");
  copieVisee = null;
}

$("sc-cancel").addEventListener("click", sftpFermerCopie);
$("sftp-copier-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const s = sftpSession();
  const visee = copieVisee;
  if (!s || !visee) return;
  const idCible = Number(($("sc-cible") as HTMLSelectElement).value);
  const cible = state.sessions.get(idCible);
  // La cible peut mourir pendant que la modale est ouverte (pty-closed marque
  // `closed`, une fermeture d'onglet la retire du magasin) : on re-teste au
  // moment de la soumission plutôt que de créer une ligne de transfert vouée à
  // l'échec. Trouvé par l'audit du 7 septembre 2026.
  if (!cible || cible.serie || cible.closed) { $("sc-error").textContent = t("sftp-aucune-autre-session"); $("sc-error").hidden = false; return; }
  const dossier = ($("sc-dossier") as HTMLInputElement).value.trim() || ".";
  const direct = ($("sc-direct") as HTMLInputElement).checked;
  const remote = remoteJoin(visee.path, visee.entry.name);
  const libelle = cible.tab.querySelector(".label")?.textContent ?? String(idCible);
  sftpFermerCopie();
  ajouterTransfert("copie", `${visee.entry.name} → ${libelle}`, cible, async (transfert) => {
    await invoke<string>("sftp_copier_vers", { id: s.id, transfert, remote, isDir: visee.entry.is_dir, idCible, remoteDirCible: dossier, direct });
    return direct ? t("sftp-copie-directe-faite") : t("sftp-copie-faite", { cible: libelle });
  }, direct);
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("sftp-copier-modal").classList.contains("open")) {
    // Même garde que le flux d'envoi des snippets : sans stopImmediatePropagation,
    // le gestionnaire d'Échap de menu-hote fermait aussi la surface du dessous.
    // Trouvé par l'audit du 7 septembre 2026 (fragilité latente : cette modale
    // s'ouvre toujours par-dessus une autre).
    e.stopImmediatePropagation();
    sftpFermerCopie();
  }
});

async function sftpPickAndUpload() {
  if (!sftpSession()) return;
  // La boîte de sélection est ouverte par le natif, qui retient les chemins
  // choisis : seuls ceux-là sont ensuite acceptés à l'envoi (audit du
  // 9 septembre 2026, voir `commands::choix_locaux`).
  let picked: string[];
  try {
    picked = await invoke<string[]>("choisir_fichiers_locaux", { titre: t("sftp-fichiers-a-envoyer"), dossiers: false });
  } catch (e) {
    sftpStatus("⚠️ " + t("selecteur-indisponible", { e: String(e) }), "err");
    return;
  }
  if (picked.length === 0) return;
  sftpUploadPaths(picked);
}

async function sftpMkdir(dir: string) {
  const s = sftpSession();
  if (!s) return;
  const name = await askText(t("nouveau-dossier-2"), t("dossiers-nom"), "");
  if (name === null) return;
  if (!validFileName(name)) {
    sftpStatus(t("sftp-nom-dossier-invalide"), "err");
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
  const name = await askText(t("sftp-renommer"), t("sftp-nouveau-nom"), entry.name);
  if (name === null || name === entry.name) return;
  if (!validFileName(name)) {
    sftpStatus(t("sftp-nom-invalide"), "err");
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
  const what = entry.is_dir ? t("sftp-le-dossier-vide", { nom: entry.name }) : `« ${entry.name} »`;
  if (!(await askConfirm(t("sftp-supprimer-question", { quoi: what })))) return;
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
    const needsEntry = ["download", "copy-to", "rename", "delete", "copy"].includes(act);
    const dirOnly = act === "cd";
    item.hidden = (needsEntry && !entry) || (dirOnly && !!entry && !entry.is_dir);
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
  if (act === "download" && entry) sftpDownload(full, entry.name, entry.is_dir);
  else if (act === "copy-to" && entry) sftpOuvrirCopie(entry, path);
  else if (act === "cd") {
    const target = entry?.is_dir ? full : path;
    invoke("pty_write", { id: s.id, data: `cd ${shellQuote(target)}\r` }).catch(() => {});
    s.term.focus();
  } else if (act === "copy") {
    // Le rejet de writeText (WebKitGTK : permission, contexte) etait avale : le
    // chemin n'etait pas copie et rien ne le disait. On le signale desormais,
    // comme le fait la copie de cle publique (audit du 7 septembre 2026).
    navigator.clipboard.writeText(full).then(
      () => sftpStatus(t("sftp-chemin-copie", { chemin: full }), "ok"),
      () => sftpStatus(t("enregistrements-copie-impossible"), "err"),
    );
  } else if (act === "rename" && entry) void sftpRename(entry, path);
  else if (act === "mkdir") void sftpMkdir(path);
  else if (act === "delete" && entry) void sftpDelete(entry, path);
});

$("sftp-list").addEventListener("contextmenu", (e) => {
  // `:not(.up)` : sur l'entrée « .. », les deux écouteurs se neutralisaient —
  // la délégation l'écarte, celui-ci sortait tôt. Résultat : aucun menu Avash
  // ET aucun `preventDefault`, donc le menu natif de WebKitGTK pouvait
  // apparaître par-dessus le panneau. « .. » reçoit maintenant le menu du
  // répertoire courant, ce qui est la bonne réponse.
  if ((e.target as HTMLElement).closest(".sftp-entry:not(.up)")) return;
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

listen<{ id: number; transfert: number; name: string; kind: string; fichier: string; done: number; total: number; termines: number; nombre: number }>("sftp-progress", (ev) => {
  progressionTransfert(ev.payload);
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
