// Bureaux RDP : sessions (canvas), entrées, presse-papiers, bureaux enregistrés.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ic } from "./icons";
import { partageClipboard, sonBureau } from "./prefs";
import { LecteurAudio } from "./audio";
import { rdpScancode, le16, rdpMousePos, humanSize, tailleBureau } from "./filters";
import { langue } from "./i18n";
import { FiltreCtrlAltGrWindows, estWindows, keysymDe, messageKeysym } from "./vnc-clavier";
import { ToucheTenues } from "./touches-tenues";
import { $, type RdpHostT, state } from "./etat";
import { askConfirm, askPassword } from "./dialogues";
import { majMemoireOnglets } from "./onglets-restauration";
import { appliquerVue, estAffiche, surFermeture, surFocus } from "./vue-partagee";
import { closeAllContextMenus, placerMenu } from "./menu-hote";
import { currentLocks } from "./verrous";
import { focusTab, orderedTabs } from "./raccourcis";
import { loadHosts, renderHosts } from "./main";
import { notify, notifyErreur } from "./notifications";
import { openMoveModal } from "./dossiers";
import { sftp, sftpAppliquerVue } from "./sftp";
import { t } from "./i18n";

// ---------- RDP (bureau distant, via le sidecar avash-rdp) ----------


type RdpTarget = { host: string; port: number | null; user: string; password: string; width?: number; height?: number; hostId?: string; name?: string; sansNla?: boolean; vnc?: boolean; partage?: string };

/** Le protocole d'un bureau enregistré, tel que le cœur le nomme. */
const protocoleDe = (h: RdpHostT): "rdp" | "vnc" => (h.protocole === "vnc" ? "vnc" : "rdp");

/** Serveurs pour lesquels on a accepté de se passer de NLA, le temps de la
 *  session. Un bureau enregistré, lui, retient ce choix dans son fichier. */
const sansNlaAccepte = new Set<string>();

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
// Quand le presse-papiers du poste part vers le bureau distant (message [8]).
// RDP : [8] fait annoncer le format au serveur, qui peut alors réclamer le texte
// tout de suite. On ne l'envoie donc que sur un geste réel dans le bureau — un
// clic dans le canvas (mousedown) —, jamais au simple retour de la fenêtre, à la
// connexion, ni à une bascule d'onglet (Ctrl+Tab, Ctrl+1..9, clic d'onglet) : le
// focus n'est pas un geste (il se déclenche aussi au retour de la fenêtre), et
// sinon un mot de passe fraîchement copié partait vers tout serveur RDP ouvert
// qu'on ne faisait que traverser. Le serveur qui réclame ensuite
// (on_request_format_list) n'a du texte qu'après une telle annonce.
// VNC : [8] ne fait que mémoriser le texte côté sidecar ; rien ne part au serveur
// tant que l'utilisateur ne colle pas explicitement (Ctrl+V/Maj+Inser -> [22]).
// L'annonce peut donc y rester liée au focus, sans fuite.
// Trouvé par l'audit du 7 septembre 2026 : SECURITY.md promet « sur un geste dans
// le bureau distant », le code poussait au focus, à la connexion et à la bascule.
export function pousseAuGeste(vnc: boolean, evenement: "focus" | "mousedown" | "connexion" | "bascule"): boolean {
  return vnc ? evenement !== "mousedown" : evenement === "mousedown";
}
/** Ce que le bureau distant a copié en dernier : liste et total, tels que le
 *  processus les annonce (message [15]). Rien n'est téléchargé avant l'accord. */
type FichiersDistants = { dossier: string; octets: number; fichiers: { chemin: string; taille: number; dossier: boolean }[] };

export const rdpSessions = new Map<number, { canvas: HTMLCanvasElement; tab: HTMLElement; ws: WebSocket | null; ro?: ResizeObserver; detachRect?: () => void; hostId?: string; syncSize?: () => void; target?: RdpTarget; badge?: HTMLElement; fichiers?: FichiersDistants | null; reception?: boolean; audio?: LecteurAudio }>();

/** Envoie un message JSON au processus (fichiers par le presse-papiers). */
function envoyerJson(ws: WebSocket | null, code: number, valeur: unknown): boolean {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  const corps = new TextEncoder().encode(JSON.stringify(valeur));
  const m = new Uint8Array(1 + corps.length);
  m[0] = code;
  m.set(corps, 1);
  ws.send(m);
  return true;
}

/** L'onglet de bureau distant actif, s'il y en a un. */
export function bureauActif(): number | null {
  return state.active !== null && rdpSessions.has(state.active) ? state.active : null;
}

/** Le bureau actif a-t-il des fichiers copiés à recevoir ? */
export function fichiersARecevoir(): boolean {
  const id = bureauActif();
  return id !== null && !!rdpSessions.get(id)?.fichiers;
}

/** Demande à recevoir ce que le bureau distant a copié, après confirmation :
 *  la liste (noms, tailles) est déjà là, le contenu ne vient qu'ensuite. */
export async function recevoirFichiers(id: number): Promise<void> {
  const s = rdpSessions.get(id);
  if (!s) return;
  const f = s.fichiers;
  if (!f) { notify(t("rdp-fichiers-aucun")); return; }
  if (s.reception) return;
  const ok = await askConfirm(
    `${t("rdp-fichiers-recevoir-titre")}\n\n${t("rdp-fichiers-recevoir-detail", { n: f.fichiers.length, taille: humanSize(f.octets, langue()), dossier: f.dossier })}`,
    { ok: t("rdp-fichiers-recevoir-bouton"), danger: false },
  );
  if (!ok) return;
  if (envoyerJson(s.ws, 16, {})) {
    s.reception = true;
    if (s.badge) { s.badge.classList.add("en-cours"); s.badge.textContent = "⬇︎ …"; }
  }
}

/** Propose des fichiers du poste au bureau distant : ils se collent là-bas. */
function offrirFichiers(id: number, chemins: string[]): void {
  const s = rdpSessions.get(id);
  if (!s || chemins.length === 0) return;
  envoyerJson(s.ws, 19, chemins);
}

/** Sélecteur de fichiers, puis offre au bureau actif. */
export async function choisirEtOffrirFichiers(): Promise<void> {
  const id = bureauActif();
  if (id === null) return;
  let choisis: string[] | string | null;
  try {
    choisis = await openDialog({ multiple: true, directory: false, title: t("rdp-fichiers-a-envoyer") });
  } catch (e) {
    notifyErreur(t("selecteur-indisponible", { e: String(e) }));
    return;
  }
  if (!choisis) return;
  offrirFichiers(id, Array.isArray(choisis) ? choisis : [choisis]);
}

/** Bilan d'une réception ou d'une offre (message [18]). */
async function bilanFichiers(id: number, b: { sens: string; dossier?: string; fichiers: number; octets: number; erreurs: string[] }): Promise<void> {
  const s = rdpSessions.get(id);
  if (!s) return;
  if (b.sens === "offre") {
    if (b.erreurs.length) notifyErreur(t("rdp-fichiers-offre-impossible", { e: b.erreurs.join(" · ") }));
    else notify(t("rdp-fichiers-offerts", { n: b.fichiers, taille: humanSize(b.octets, langue()) }), "succes");
    return;
  }
  s.reception = false;
  s.fichiers = null;
  if (s.badge) { s.badge.hidden = true; s.badge.classList.remove("en-cours"); }
  if (b.erreurs.length) notifyErreur(t("rdp-fichiers-erreurs", { e: b.erreurs.join(" · ") }));
  const ouvrir = await askConfirm(
    `${t("rdp-fichiers-recus-titre", { n: b.fichiers })}\n\n${t("rdp-fichiers-recus-detail", { taille: humanSize(b.octets, langue()), dossier: b.dossier ?? "" })}`,
    { ok: t("rdp-ouvrir-dossier"), danger: false },
  );
  if (ouvrir && b.dossier) await invoke("rdp_ouvrir_dossier", { chemin: b.dossier }).catch((e) => notifyErreur(String(e)));
}

/** Décide de la taille à demander au serveur pour un redimensionnement natif du
 *  bureau distant (message [5], Display Control DVC). Rend `[largeur, hauteur]`
 *  bornée (largeur paire, 200..8192) ou `null` s'il n'y a rien à faire.
 *
 *  Trouvé par l'audit du 7 septembre 2026 :
 *  - VNC (RFB) n'a aucun canal Display Control : le sidecar (vnc.rs) n'a pas de
 *    bras pour [5] et ne renvoie jamais de [1] de confirmation. Poser un drapeau
 *    « en vol » puis rejouer au filet des 3 s y tournerait en boucle permanente ;
 *    on n'y redimensionne donc pas (le canvas suit la fenêtre en CSS de toute
 *    façon).
 *  - un volet caché ne se redimensionne pas ; un écart de moins de 8 px est du
 *    bruit de sous-pixel du glissé, négligeable. */
export function prochainRedimensionnement(
  vnc: boolean,
  affiche: boolean,
  largeurConteneur: number,
  hauteurConteneur: number,
  largeurActuelle: number,
  hauteurActuelle: number,
  dpr: number = window.devicePixelRatio,
): [number, number] | null {
  if (vnc || !affiche) return null;
  // `largeurActuelle`/`hauteurActuelle` sont la taille confirmée par le serveur
  // (message CONNECTED), donc des pixels PHYSIQUES. On calcule la cible dans la
  // MÊME unité (via `tailleBureau`, qui multiplie le rect CSS par le DPR) : sinon
  // le seuil « négligeable » ci-dessous comparait des pixels CSS à des pixels
  // physiques et, à DPR=2, l'écart valait la moitié de la définition — sendResize
  // repartait à chaque ResizeObserver et le serveur renégociait en boucle.
  // Trouvé par l'audit du 7 septembre 2026 (HiDPI).
  const [w, h] = tailleBureau({ width: largeurConteneur, height: hauteurConteneur }, dpr);
  if (Math.abs(w - largeurActuelle) < 8 && Math.abs(h - hauteurActuelle) < 8) return null;
  return [w, h];
}

/** Faut-il redemander au sidecar l'image entière du bureau ([9]) quand un onglet
 *  RDP prend le focus ?
 *
 *  Trouvé par l'audit du 7 septembre 2026 : `focusRdp` envoyait [9] à CHAQUE
 *  activation d'onglet, sans regarder si le canvas était déjà affiché. Le
 *  sidecar répond par une trame de l'image entière hors cadencement (8,3 Mo en
 *  1080p, 33 Mo en 4K), poussée sur la boucle locale puis peinte d'un
 *  `putImageData` plein écran — payé pour rien, y compris quand l'utilisateur
 *  clique l'onglet DÉJÀ actif, qui n'a rien perdu.
 *
 *  On ne rafraîchit donc que si le canvas a pu perdre son contenu :
 *  - il passait de caché à visible (`!etaitAffiche`) : le backing-store d'un
 *    canvas resté en `display:none` peut avoir été vidé par WebKitGTK ;
 *  - son conteneur a été reparenté (`aEteReparente`) : `appliquerVue` détruit et
 *    recrée les `.volet` à chaque appel, donc en vue partagée le canvas est
 *    brièvement hors document alors que son `display` n'a jamais valu « none » —
 *    c'est justement là que le backing-store est le plus susceptible d'être
 *    perdu. Se fier au seul `display` supprimerait le rafraîchissement dans ce
 *    cas et rejouerait le flash noir consigné plus haut.
 *
 *  Un canvas déjà affiché et non reparenté n'a rien perdu : pas de [9]. */
export function doitRafraichir(etaitAffiche: boolean, aEteReparente: boolean, wsOuverte: boolean): boolean {
  return wsOuverte && (!etaitAffiche || aEteReparente);
}

export async function openRdp(cible: RdpTarget) {
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
  tab.querySelector(".label")!.textContent = cible.name ?? (cible.user ? `${cible.user}@${cible.host}` : cible.host);
  tab.querySelector(".close")!.innerHTML = ic("x");
  tabs.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
  tabs.appendChild(tab);

  // Résolution = taille de la zone disponible d'Avash (adaptatif), sauf si
  // une taille précise est imposée. RDP : largeur paire, bornes 200..8192.
  const area = $("terminal").getBoundingClientRect();
  const even = (n: number) => n - (n % 2);
  // Mutables : au redimensionnement natif, le serveur renvoie la vraie taille
  // (message CONNECTED) et on les remet à jour — le mappage souris suit.
  //
  // HiDPI (audit du 7 septembre 2026) : la zone adaptative est mesurée en pixels
  // CSS ; on la convertit en pixels PHYSIQUES (`tailleBureau`, ×DPR) pour ne pas
  // négocier un bureau flou à 200 %. Une taille IMPOSÉE par un bureau enregistré
  // est déjà en pixels du bureau : on la borne telle quelle, sans DPR. VNC : on
  // ne double PAS (dpr=1) — le protocole RFB n'a pas d'équivalent du
  // `desktop_scale_factor` de RDP, donc doubler la définition rétrécirait le
  // texte distant sans compensation possible ; on préfère l'état actuel (grand,
  // quitte à être un peu flou) à un texte net mais deux fois plus petit.
  let rdpW: number;
  let rdpH: number;
  if (cible.width && cible.height) {
    rdpW = Math.max(200, Math.min(8192, even(Math.round(cible.width))));
    rdpH = Math.max(200, Math.min(8192, Math.round(cible.height)));
  } else {
    [rdpW, rdpH] = tailleBureau(
      { width: area.width || 1280, height: area.height || 800 },
      cible.vnc ? 1 : window.devicePixelRatio,
    );
  }

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
  hud.title = t("rdp-hud-titre");
  hud.addEventListener("click", () => hud.classList.toggle("mini"));
  // Pastille des fichiers copiés sur le bureau distant : un clic pour recevoir.
  const badge = document.createElement("button");
  badge.type = "button";
  badge.className = "rdp-fichiers";
  badge.hidden = true;
  badge.addEventListener("click", () => void recevoirFichiers(id));
  wrap.appendChild(canvas);
  wrap.appendChild(hud);
  wrap.appendChild(badge);
  $("terminal").appendChild(wrap);
  // Canvas LOGICIEL, pas accéléré : `willReadFrequently` fait vivre le bitmap
  // en mémoire centrale. Un canvas 2D accéléré est une texture GPU, et sous
  // une session RDP le GPU est virtualisé : à chaque perte de contexte (une
  // reconnexion suffit), Chromium efface la texture et ne repeint ensuite que
  // les rectangles que le serveur renvoie — le reste restait NOIR, jusqu'à ce
  // qu'un survol ou une frappe fasse renvoyer la zone. Reproduit le
  // 2026-09-03 sur un avash Windows 0.6.2 ouvert dans une session RDP, où le
  // bouton OK d'un avertissement de connexion manquait jusqu'au passage de la
  // souris, dans notre client comme dans FreeRDP : la tuile était perdue côté
  // WebView2, pas au décodage. On ne fait que des putImageData : le chemin
  // logiciel est le bon de toute façon.
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  rdpSessions.set(id, { canvas, tab, ws: null, hostId: cible.hostId, target: cible, badge, fichiers: null });
  majMemoireOnglets();
  state.active = id;

  tab.addEventListener("click", () => focusRdp(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => { e.stopPropagation(); closeRdp(id); });

  // Souris/clavier → sidecar via le WebSocket (binaire). Ignore si non prêt.
  const send = (bytes: number[]) => {
    const s = rdpSessions.get(id);
    if (s?.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array(bytes));
  };
  // Touches et boutons tenus enfoncés dans CETTE session, pour tout relâcher si
  // le canvas perd le focus alors qu'une touche l'est encore (Alt+Tab, Super,
  // ou Ctrl+Tab — le raccourci d'onglet d'Avash) : sinon le keyup part à l'autre
  // fenêtre et la touche reste tenue sur le bureau distant. Audit du 7 sept 2026.
  const tenues = new ToucheTenues(
    cible.vnc
      ? (jeton) => messageKeysym(Number(jeton), false)
      : (jeton) => { const sc = rdpScancode(jeton); return sc ? [4, ...le16(sc), 0] : null; },
  );
  const boutonsTenus = new Set<number>();
  let moveX = 0, moveY = 0; // dernière position souris connue (relâchement d'un bouton au blur)
  // Relâche tout ce qui est tenu (touches puis boutons souris) et vide le suivi.
  const relacherTenues = () => {
    for (const m of tenues.relacherTout()) send(m);
    // Un glissé commencé sur le canvas et relâché ailleurs (barre d'onglets,
    // panneau SFTP, hors de la fenêtre) ne produit jamais de mouseup ici : le
    // bouton resterait enfoncé côté distant. Message [2] bouton, relâché.
    for (const b of boutonsTenus) send([2, b, 0, ...le16(moveX), ...le16(moveY)]);
    boutonsTenus.clear();
  };
  // L'onglet passe en arrière-plan (fenêtre minimisée, autre onglet système) :
  // les keyup ne viendront plus. On relâche comme au blur.
  const surVisibiliteCachee = () => { if (document.hidden) relacherTenues(); };
  document.addEventListener("visibilitychange", surVisibiliteCachee);
  // Mappage souris -> pixels du bureau (letterbox object-fit:contain), testé.
  // getBoundingClientRect force un recalcul de mise en page synchrone : l'appeler
  // à CHAQUE mousemove (jusqu'à 1000/s sur une souris rapide) rivalisait avec les
  // putImageData de la même trame. On mémorise le rect et on ne l'invalide que
  // lorsque la géométrie change réellement (redimensionnement, défilement, focus).
  let rectCache: DOMRect | null = null;
  const rectCanvas = (): DOMRect => (rectCache ??= canvas.getBoundingClientRect());
  const invaliderRect = () => { rectCache = null; };
  const pos = (e: MouseEvent): [number, number] =>
    rdpMousePos(e.clientX, e.clientY, rectCanvas(), rdpW, rdpH);
  // Le rect bouge avec la fenêtre et le défilement de la page ; on l'oublie alors.
  window.addEventListener("resize", invaliderRect);
  window.addEventListener("scroll", invaliderRect, true);
  // Retirés à la fermeture de l'onglet : sans quoi ils s'accumuleraient à chaque
  // bureau ouvert puis fermé.
  let detacherDpr: (() => void) | null = null; // watcher HiDPI, armé plus bas
  const detachRect = () => {
    window.removeEventListener("resize", invaliderRect);
    window.removeEventListener("scroll", invaliderRect, true);
    document.removeEventListener("visibilitychange", surVisibiliteCachee);
    detacherDpr?.();
  };
  // Mouvements souris throttlés au rAF : un seul paquet par frame d'affichage.
  let movePending = false;
  canvas.addEventListener("mousemove", (e) => {
    [moveX, moveY] = pos(e);
    if (movePending) return;
    movePending = true;
    requestAnimationFrame(() => { movePending = false; send([1, ...le16(moveX), ...le16(moveY)]); });
  });
  canvas.addEventListener("mousedown", (e) => { e.preventDefault(); canvas.focus(); const [x, y] = pos(e); boutonsTenus.add(e.button); send([2, e.button, 1, ...le16(x), ...le16(y)]);
    // Un clic dans le bureau = geste réel : on annonce le presse-papiers du poste
    // (le seul chemin qui le fait en RDP). Forcé pour couvrir un second bureau
    // dont le sidecar n'a pas encore reçu ce texte. Cf. `pousseAuGeste`.
    if (pousseAuGeste(cible.vnc === true, "mousedown")) void pushLocalClipboard(true);
  });
  canvas.addEventListener("mouseup", (e) => { const [x, y] = pos(e); boutonsTenus.delete(e.button); send([2, e.button, 0, ...le16(x), ...le16(y)]); });
  // Clic droit : uniquement pour le bureau distant. On empêche le menu du
  // navigateur ET la remontée vers #terminal (qui ouvrirait le menu d'Avash).
  canvas.addEventListener("contextmenu", (e) => { e.preventDefault(); e.stopPropagation(); });
  canvas.addEventListener("wheel", (e) => { e.preventDefault(); const d = e.deltaY > 0 ? -120 : 120; send([3, ...le16(d & 0xffff), 0, 0, 0, 0]); });
  // Traitement VNC réel d'une touche (keysym, suivi des tenues, envoi). Passé au
  // filtre Ctrl/AltGr pour qu'il puisse différer ou abandonner un keydown.
  const emettreVnc = (e: { key: string; code: string }, enfonce: boolean) => {
    const ks = keysymDe(e);
    if (ks === null) return;
    const jeton = String(ks);
    if (enfonce) tenues.enfoncer(jeton);
    // Un keyup sans keydown dans cette session (le relâchement de Ctrl après
    // un Ctrl+Tab atterrit sur le nouveau canvas) ne doit rien envoyer : le
    // serveur VNC recevrait un release fantôme d'une touche jamais pressée.
    else if (!tenues.relacher(jeton)) return;
    send(messageKeysym(ks, enfonce));
  };
  // Sous Windows, AltGr est émulé par un Ctrl gauche synthétique suivi d'Alt
  // droite : sans ce filtre, « @ # { } | \ ~ € » arrivaient comme Ctrl+caractère
  // sur un serveur VNC X11. Trouvé par l'audit du 7 septembre 2026. RDP (scancodes)
  // et Linux (vraie ISO_Level3_Shift) ne sont pas concernés.
  const filtreClavier = new FiltreCtrlAltGrWindows(cible.vnc === true && estWindows(), emettreVnc);
  // RDP transporte la touche physique (scancode), VNC le caractère obtenu
  // (keysym) : même écouteur, deux messages.
  const touche = (e: KeyboardEvent, enfonce: boolean) => {
    if (cible.vnc) {
      filtreClavier.traiter(e, enfonce);
    } else {
      const sc = rdpScancode(e.code);
      if (!sc) return;
      // RDP : ironrdp filtre déjà un release non pressé (was_pressed=false), on
      // envoie donc comme avant ; on tient juste le suivi à jour pour le blur.
      if (enfonce) tenues.enfoncer(e.code);
      else tenues.relacher(e.code);
      send([4, ...le16(sc), enfonce ? 1 : 0]);
    }
  };
  // Un collage local -> distant en VNC (Ctrl+V ou Maj+Inser). En RFB il n'y a
  // pas de phase de demande comme en RDP : le presse-papiers du poste ne part au
  // serveur que sur ce geste explicite. Le sidecar mémorise l'annonce [8] (le
  // focus la pousse) et n'émet le ClientCutText que sur ce message [22].
  const estCollageVnc = (e: KeyboardEvent): boolean =>
    cible.vnc === true && ((e.ctrlKey && e.code === "KeyV") || (e.shiftKey && e.code === "Insert"));
  canvas.addEventListener("keydown", (e) => {
    if (e.code === "F11") { e.preventDefault(); return; } // géré globalement (plein écran)
    e.preventDefault();
    // Émis AVANT la frappe : l'ordre FIFO du WebSocket garantit que le texte est
    // posé côté serveur avant la touche qui le colle. Trouvé par l'audit du
    // 7 septembre 2026 : sans geste explicite, le VNC envoyait le presse-papiers
    // du poste au serveur dès la connexion et à chaque focus (fuite).
    if (estCollageVnc(e)) send([22]);
    // Pas de resynchronisation ici : le navigateur ne sait pas lire ces verrous
    // sous WebKitGTK, et renvoyer sa valeur éteindrait le pavé numérique du
    // distant dès la première frappe. Verr.Num est de toute façon transmise
    // comme n'importe quelle touche : le bureau distant bascule lui-même.
    touche(e, true);
  });
  canvas.addEventListener("keyup", (e) => { e.preventDefault(); touche(e, false); });
  // Le canvas perd le focus (Alt+Tab, changement d'onglet, clic sur la barre
  // d'onglets ou le panneau SFTP) : les keyup/mouseup restants partiront
  // ailleurs. On relâche tout de suite pour ne rien laisser tenu côté distant.
  canvas.addEventListener("blur", relacherTenues);
  // Au focus du canvas : le rect est peut-être périmé (onglet redevenu visible)
  // et les verrous du poste sont à resynchroniser. Le presse-papiers, lui, ne
  // part qu'en VNC (le sidecar le retient jusqu'au collage) : en RDP, le focus
  // n'est PAS un geste — il survient aussi au retour de la fenêtre — et [8] y
  // ferait fuir un mot de passe fraîchement copié. Cf. `pousseAuGeste`.
  canvas.addEventListener("focus", () => {
    invaliderRect(); // l'onglet vient (peut-être) de devenir visible : rect à relire
    void currentLocks().then((l) => { if (l !== null) send([10, l]); });
    if (pousseAuGeste(cible.vnc === true, "focus")) void pushLocalClipboard(true);
  });

  // Redimensionnement NATIF du bureau distant : quand la zone Avash change, on
  // demande au serveur de re-rendre à la nouvelle taille (Display Control DVC).
  // Débounce pour ne pas spammer pendant le glissé de la fenêtre. Message [5].
  let resizeTimer: number | undefined;
  let resizeInFlight = false; // une seule renégociation RDP à la fois
  let resizeGuard: number | undefined;
  const sendResize = () => {
    // Seul un bureau affiché se redimensionne : l'actif, ou l'autre volet de la
    // vue partagée. La taille est celle de son conteneur, pas de toute la zone.
    const a = wrap.getBoundingClientRect();
    const taille = prochainRedimensionnement(cible.vnc === true, estAffiche({ kind: "rdp", id }), a.width, a.height, rdpW, rdpH);
    if (!taille) return;
    if (resizeInFlight) return; // une renégociation en cours ; le filet rejouera la taille courante
    resizeInFlight = true;
    window.clearTimeout(resizeGuard);
    // Filet : si le serveur ne confirme pas par [1] (son canal Display Control
    // n'avait pas encore reçu ses capacités quand le [5] est parti — le sidecar
    // l'ignore alors en silence), on relâche le drapeau ET on rejoue la taille
    // courante. Sans ce rejeu, un redimensionnement demandé avant l'ouverture du
    // canal restait perdu jusqu'à ce que l'utilisateur bouge lui-même la fenêtre
    // (bureau letterboxé). Trouvé par l'audit du 7 septembre 2026.
    resizeGuard = window.setTimeout(() => { resizeInFlight = false; sendResize(); }, 3000);
    send([5, ...le16(taille[0]), ...le16(taille[1])]);
  };
  const ro = new ResizeObserver(() => {
    invaliderRect(); // la zone a changé de taille : le rect mémorisé est périmé
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(sendResize, 400);
  });
  // Le conteneur, pas la zone : dans un volet, c'est lui qui a la bonne taille.
  ro.observe(wrap);
  // Un changement de ratio de pixels — fenêtre déplacée d'un écran 100 % vers un
  // écran 200 %, ou échelle du bureau modifiée — laisse la taille CSS identique :
  // le ResizeObserver ne se déclenche pas et la session resterait à l'ancienne
  // définition (floue ou trop grande). On écoute donc la résolution elle-même et
  // on rejoue le calcul. `matchMedia` est figé sur le dppx courant, donc on le
  // réarme sur le nouveau à chaque changement. Trouvé par l'audit du 7 septembre
  // 2026 (HiDPI).
  let mqDpr: MediaQueryList | null = null;
  const surChangementDpr = () => {
    invaliderRect();
    sendResize();
    armerMediaDpr();
  };
  const armerMediaDpr = () => {
    mqDpr?.removeEventListener("change", surChangementDpr);
    mqDpr = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    mqDpr.addEventListener("change", surChangementDpr);
  };
  armerMediaDpr();
  detacherDpr = () => mqDpr?.removeEventListener("change", surChangementDpr);
  rdpSessions.get(id)!.ro = ro;
  rdpSessions.get(id)!.detachRect = detachRect;
  rdpSessions.get(id)!.syncSize = sendResize;

  // Bureau reçu via WebSocket local BINAIRE (ArrayBuffer natif : ni base64 ni
  // JSON — débit maximal, même en 3440×1440).
  //   [1] CONNECTED w,h · [2] FRAME x,y,w,h + RGBA · [3] ERROR utf8
  try {
    const conn = await invoke<{ port: number; token: string }>("rdp_open", {
      id, host: cible.host, port: cible.port, user: cible.user, password: cible.password,
      width: rdpW, height: rdpH,
      // HiDPI (audit du 7 septembre 2026) : on négocie la taille en pixels
      // physiques ; il faut alors annoncer l'échelle au serveur RDP
      // (`desktopScaleFactor`, MS-RDPBCGR : 100..500) pour qu'il rende son
      // interface plus grande, comme mstsc. Sans cela le texte distant serait net
      // mais deux fois plus petit à 200 %. VNC ignore ce champ (pas d'équivalent).
      desktopScaleFactor: Math.round(window.devicePixelRatio * 100),
      sansNla: cible.sansNla === true || sansNlaAccepte.has(`${cible.host}:${cible.port ?? 3389}`),
      vnc: cible.vnc === true,
      sansSon: !sonBureau(),
      partage: cible.partage ?? null,
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
      // Annonce initiale du presse-papiers seulement en VNC (le sidecar le retient
      // jusqu'au collage explicite). En RDP, rien à la connexion : l'annoncer sans
      // geste ferait fuir le presse-papiers vers un serveur qu'on vient d'ouvrir.
      if (pousseAuGeste(cible.vnc === true, "connexion")) window.setTimeout(() => void pushLocalClipboard(), 600);
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
      } else if (kind === 13) {
        // Trame à plusieurs rectangles. Le sidecar n'accumulait qu'une union
        // englobante : deux petites zones aux coins opposés donnaient un
        // rectangle plein écran. Mesuré contre un vrai xrdp, 1,8 fois trop
        // d'octets. Une seule trame, donc un seul accusé : le cadencement
        // reste exact.
        try {
          const n = dv.getUint8(1);
          let p = 2;
          for (let i = 0; i < n; i++) {
            const x = dv.getUint16(p, true), y = dv.getUint16(p + 2, true);
            const fw = dv.getUint16(p + 4, true), fh = dv.getUint16(p + 6, true);
            p += 8;
            ctx.putImageData(
              new ImageData(new Uint8ClampedArray(buf, p, fw * fh * 4), fw, fh), x, y);
            p += fw * fh * 4;
          }
        } catch (err) {
          console.warn("trame RDP multiple invalide", err);
        }
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
          snap.getContext("2d", { willReadFrequently: true })!.drawImage(canvas, 0, 0);
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
      } else if (kind === 20 || kind === 21) {
        // Son du distant : blocs PCM joués à la suite, volume demandé.
        const s = rdpSessions.get(id);
        if (s) {
          s.audio ??= new LecteurAudio();
          if (kind === 20) s.audio.jouer(buf);
          else if (buf.byteLength >= 5) s.audio.volume(dv.getUint16(1, true), dv.getUint16(3, true));
        }
      } else if (kind === 15 || kind === 17 || kind === 18) {
        // Fichiers par le presse-papiers : la liste copiée sur le distant, la
        // progression d'une réception, le bilan d'une réception ou d'une offre.
        let corps: unknown;
        try { corps = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 1))); } catch { return; }
        const s = rdpSessions.get(id);
        if (!s) return;
        if (kind === 15) {
          const f = corps as FichiersDistants;
          s.fichiers = f;
          s.reception = false;
          badge.classList.remove("en-cours");
          badge.textContent = t(f.fichiers.length === 1 && !f.fichiers[0].dossier ? "rdp-fichier-copie" : "rdp-fichiers-copies", { n: f.fichiers.length, taille: humanSize(f.octets, langue()) });
          badge.hidden = false;
        } else if (kind === 17) {
          const p = corps as { fichier: string; fait: number; total: number; termines: number; nombre: number };
          badge.classList.add("en-cours");
          badge.textContent = t("rdp-fichiers-en-cours", { fichier: p.fichier, fait: humanSize(p.fait, langue()), total: humanSize(p.total, langue()), termines: p.termines, nombre: p.nombre });
          badge.hidden = false;
        } else {
          void bilanFichiers(id, corps as { sens: string; dossier?: string; fichiers: number; octets: number; erreurs: string[] });
        }
      } else if (kind === 14) {
        // Le presse-papiers distant a changé (texte ou autre format) : la liste
        // de fichiers offerte est caduque, ses verrous côté serveur vont
        // expirer. On efface la pastille et l'état pour ne plus proposer une
        // réception vouée à l'échec. Sauf réception en cours : le sidecar ne
        // l'envoie déjà pas dans ce cas, garde-fou ici aussi pour ne pas
        // effacer la pastille « ⬇︎ … » d'un transfert légitime. Trouvé par
        // l'audit du 7 septembre 2026.
        const s = rdpSessions.get(id);
        if (s && !s.reception) {
          s.fichiers = null;
          badge.hidden = true;
          badge.classList.remove("en-cours");
        }
      }
    };
    ws.onclose = () => {
      const st = tab.querySelector(".state");
      if (st) st.className = "state closed";
      tab.classList.add("dead");
      // Le processus RDP, l'observateur de taille, les écouteurs d'invalidation
      // de rect (resize/scroll/visibilitychange) et le contexte audio (créé au
      // premier bloc de son, message [20]) survivaient à la coupure : le
      // premier restait dans la table côté Rust jusqu'à l'arrêt de
      // l'application, le deuxième continuait d'observer #terminal pour un
      // canvas mort, les troisièmes s'accumulaient, et le dernier gardait un
      // flux de sortie ouvert sur le périphérique — l'application restait
      // listée comme lisant du son — pour un onglet mort tant qu'il n'était pas
      // fermé ou reconnecté. On relâche donc les mêmes ressources locales qu'à
      // la fermeture explicite (`terminerSession`). L'onglet et le canvas
      // restent, eux — « Reconnecter » doit rester possible. Contexte audio et
      // écouteurs de rect ajoutés par l'audit du 7 septembre 2026.
      const s = rdpSessions.get(id);
      if (s) terminerSession(s);
      // Trouvé par l'audit du 7 septembre 2026 : `rdp_close` et `rdp_diagnostic`
      // sont des commandes synchrones, exécutées en ligne côté Rust dans l'ordre
      // d'émission. Appeler `rdp_close` (qui retire le journal) avant
      // `showRdpClosed` (qui lit `rdp_diagnostic`) faisait lire un journal déjà
      // effacé : l'incrustation « Connexion RDP fermée » restait muette. On
      // montre l'incrustation d'abord, sa lecture du diagnostic part donc avant
      // la fermeture et retrouve la raison de la coupure.
      showRdpClosed(id);
      void invoke("rdp_close", { id }).catch(() => {});
    };
    ws.onerror = () => { /* onclose suivra */ };
  } catch (e) {
    // Fermeture volontaire pendant la connexion : le back le signale par un
    // marqueur. Rien à afficher, l'onglet n'existe déjà plus.
    if (String(e).includes("[AVASH_RDP_ANNULE]")) return;
    if (!rdpSessions.has(id)) return;
    // Le serveur ne sait pas faire d'authentification réseau. Ce n'est pas
    // forcément une attaque — un xrdp dont le module PAM n'est pas configuré
    // est dans ce cas —, mais ce n'est pas à nous d'en décider en silence.
    if (String(e).includes("[AVASH_RDP_SANS_NLA]") && (await proposerSansNla(cible, String(e)))) {
      closeRdp(id);
      await openRdp({ ...cible, sansNla: true });
      return;
    }
    tab.querySelector(".state")!.className = "state closed";
    notify(t("rdp-connexion-impossible", { e: String(e) }), "erreur");
    showRdpClosed(id); // proposer de réessayer
  }
  focusRdp(id);
}

/** Demande s'il faut se connecter à un serveur qui ne propose pas NLA.
 *
 *  Le message doit dire ce qu'on perd, et ce qu'on garde. Sans NLA, le mot de
 *  passe part dans le canal TLS sans que le serveur se soit authentifié auprès
 *  de nous par CredSSP. Mais Avash épingle malgré tout l'empreinte du serveur,
 *  comme il le fait pour une clé d'hôte SSH : dès la deuxième connexion, un
 *  imposteur est refusé. Le risque se limite donc au premier contact — c'est
 *  exactement le compromis du TOFU, et il faut le dire tel quel plutôt que
 *  d'agiter un avertissement vague.
 */
async function proposerSansNla(cible: RdpTarget, erreur: string): Promise<boolean> {
  // Le processus RDP distingue deux cas — le serveur refuse NLA d'emblée, ou il
  // l'annonce sans mener l'échange à terme. On reprend SA phrase plutôt que
  // d'en inventer une générique qui serait fausse dans l'un des deux cas.
  const raison = erreur.replace(/^.*\[AVASH_RDP_SANS_NLA\]\s*/s, "").trim();
  const ok = await askConfirm(
    `${cible.name ?? cible.host} — ${raison}\n\n` + t("rdp-sans-nla-explication"),
    { ok: t("rdp-se-connecter-sans-nla") },
  );
  if (!ok) return false;
  sansNlaAccepte.add(`${cible.host}:${cible.port ?? 3389}`);
  // Un bureau enregistré retient le choix ; une connexion directe ne vaut que
  // pour cette session.
  if (cible.hostId) {
    await invoke("rdp_host_set_sans_nla", { id: cible.hostId, valeur: true }).catch(() => {});
  }
  return true;
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
export function annoncerPartageClip(ws: WebSocket): void {
  if (ws.readyState === WebSocket.OPEN) ws.send(new Uint8Array([12, partageClipboard() ? 1 : 0]));
}

export function marquerVisibilite(s: { ws: WebSocket | null }, visible: boolean): void {
  if (s.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array([11, visible ? 0 : 1]));
}

/** Donne le focus clavier au bureau, et s'assure qu'il l'a bien pris.
 *
 *  Le canvas vient de passer de `display: none` à visible. Un `focus()` posé
 *  dans la même tâche que ce changement n'aboutit pas toujours — le moteur n'a
 *  pas encore calculé la disposition, et un élément sans boîte n'est pas
 *  focalisable. Le symptôme : le bureau réapparaît après la fermeture d'un
 *  autre onglet, mais les frappes ne partent nulle part.
 *
 *  On réessaie donc à l'image suivante si le focus n'a pas pris. Deux
 *  tentatives suffisent : au-delà, c'est que quelque chose d'autre le retient
 *  (une boîte de dialogue ouverte, par exemple), et le lui arracher serait pire.
 */
function donnerLeFocusAuBureau(canvas: HTMLCanvasElement): void {
  canvas.focus();
  if (document.activeElement === canvas) return;
  requestAnimationFrame(() => {
    if (canvas.isConnected && document.activeElement !== canvas) canvas.focus();
  });
}

export function focusRdp(id: number) {
  const precedent = state.active === null ? null
    : rdpSessions.has(state.active) ? { kind: "rdp" as const, id: state.active } : { kind: "ssh" as const, id: state.active };
  state.active = id;
  surFocus({ kind: "rdp", id }, precedent);
  // On relève, AVANT d'appliquer la vue, l'état du bureau qui prend le focus :
  // était-il déjà affiché, et sous quel parent ? `appliquerVue` peut le
  // reparenter (elle détruit et recrée les `.volet` en vue partagée).
  const avant = rdpSessions.get(id);
  const etaitAffiche = !!avant && (avant.canvas.parentElement as HTMLElement).style.display !== "none";
  const parentAvant = avant?.canvas.parentElement?.parentElement ?? null;
  // La vue montre l'actif (et l'autre volet), cache le reste, prévient chaque
  // bureau de sa visibilité et rattrape la taille des affichés : une session
  // inactive n'a pas suivi les redimensionnements de la fenêtre.
  appliquerVue();
  for (const [sid, s] of rdpSessions) {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    if (active) {
      donnerLeFocusAuBureau(s.canvas);
      // Un canvas caché ou reparenté peut avoir perdu son contenu (backing-store
      // WebKitGTK) : on redemande alors l'image entière (message [9]). Sinon —
      // canvas déjà affiché et non reparenté, p. ex. clic sur l'onglet déjà
      // actif — rien n'a été perdu et on économise 8,3 Mo (1080p) à 33 Mo (4K)
      // de trame hors cadencement. Trouvé par l'audit du 7 septembre 2026.
      const aEteReparente = (s.canvas.parentElement?.parentElement ?? null) !== parentAvant;
      const wsOuverte = !!s.ws && s.ws.readyState === WebSocket.OPEN;
      if (doitRafraichir(etaitAffiche, aEteReparente, wsOuverte) && s.ws) s.ws.send(new Uint8Array([9]));
    }
  }
  state.sessions.forEach((s) => { s.tab.classList.remove("active"); });
  // Bascule d'onglet : en VNC on renvoie le presse-papiers à la session qui
  // devient active (le sidecar le retient jusqu'au collage explicite, sans
  // fuite), sinon le collage local->distant ne marche pas après un changement
  // d'onglet. En RDP, NON : une bascule n'est pas un geste dans le bureau et [8]
  // ferait annoncer un mot de passe fraîchement copié à un serveur qu'on ne fait
  // que traverser (Ctrl+Tab). Le contenu part alors au premier clic dans le
  // canvas. Trouvé par l'audit du 7 septembre 2026.
  if (pousseAuGeste(rdpSessions.get(id)?.target?.vnc === true, "bascule")) void pushLocalClipboard(true);
  // Un bureau distant n'a pas de système de fichiers : on grise le bouton
  // « Fichiers (SFTP) » et on masque le panneau, hérités de l'onglet SSH
  // précédent. `sftp.open` reste la préférence, restaurée au retour sur le SSH.
  // Trouvé par l'audit du 7 septembre 2026.
  sftpAppliquerVue();
  renderHosts(); // met à jour le surlignage « sélectionné »
}

/** Relâche les ressources locales d'une session de bureau — observateur de
 *  taille, écouteurs d'invalidation de rect (resize/scroll/visibilitychange) et
 *  contexte audio — sans retirer l'onglet ni le canvas, qui doivent survivre
 *  pour « Reconnecter ». Partagé par la coupure serveur (`ws.onclose`) et la
 *  fermeture explicite (`closeRdp`).
 *
 *  Trouvé par l'audit du 7 septembre 2026 : `ws.onclose` ne coupait que
 *  l'observateur de taille ; le contexte WebAudio (créé au premier bloc de son)
 *  et les écouteurs de rect survivaient à la coupure, gardant un flux de sortie
 *  audio réservé sur le périphérique pour un onglet mort. `fermer()` étant
 *  idempotent, le second appel depuis `closeRdp` après une coupure ne coûte
 *  rien. */
export function terminerSession(s: { ro?: ResizeObserver; detachRect?: () => void; audio?: LecteurAudio }): void {
  s.ro?.disconnect();
  s.detachRect?.();
  s.audio?.fermer();
}

export function closeRdp(id: number) {
  const s = rdpSessions.get(id);
  if (!s) return;
  surFermeture({ kind: "rdp", id });
  if (document.body.classList.contains("rdp-full")) {
    document.body.classList.remove("rdp-full");
    getCurrentWindow().setFullscreen(false).catch(() => {});
  }
  terminerSession(s);
  s.ws?.close();
  invoke("rdp_close", { id }).catch(() => {});
  s.canvas.parentElement?.remove();
  s.tab.remove();
  rdpSessions.delete(id);
  majMemoireOnglets();
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
      // Pendant de `closeSession` : sans plus aucun onglet, le panneau SFTP n'a
      // rien à montrer. `closeRdp` l'oubliait, si bien qu'après être passé sur un
      // bureau RDP puis avoir fermé l'onglet SSH (state.active vaut alors l'id
      // RDP, la branche de closeSession ne s'exécute pas) puis le bureau, on
      // affichait « Aucune session » à côté d'un panneau resté ouvert. Trouvé par
      // l'audit du 7 septembre 2026.
      sftp.open = false;
      sftpAppliquerVue();
    }
  }
  renderHosts(); // éteint le voyant vert de l'hôte fermé
}

/** Bureau RDP fermé (serveur/réseau) : propose de reconnecter ou fermer l'onglet
 *  — équivalent du message « Entrée : reconnecter · Ctrl+W : fermer » du SSH. */
/** Lit le diagnostic d'un bureau fermé, en relançant tant que la réponse est vide.
 *
 *  Trouvé par l'audit du 7 septembre 2026 : la dernière ligne « Error: … »
 *  d'anyhow est écrite par le sidecar APRÈS la fermeture de la WebSocket (le
 *  poste est libéré au retour d'`executer`), donc une lecture unique et immédiate
 *  est une course perdue d'avance et rend souvent une chaîne vide. On relit
 *  quelques fois à 300 ms pour attraper cette dernière ligne.
 *
 *  `lire` et `pause` sont injectables pour les tests (invoke moqué, sans délai).
 */
export async function lireDiagnosticRdp(
  id: number,
  lire: (id: number) => Promise<string> = (i) => invoke<string>("rdp_diagnostic", { id: i }),
  pause: (ms: number) => Promise<void> = (ms) => new Promise((r) => { setTimeout(r, ms); }),
): Promise<string> {
  for (let essai = 0; essai < 3; essai++) {
    const diag = (await lire(id).catch(() => "")).trim();
    if (diag) return diag;
    if (essai < 2) await pause(300);
  }
  return "";
}

function showRdpClosed(id: number) {
  const s = rdpSessions.get(id);
  if (!s) return; // fermeture volontaire (l'onglet est déjà retiré)
  const wrap = s.canvas.parentElement as HTMLElement | null;
  if (!wrap || wrap.querySelector(".rdp-closed")) return;
  const ov = document.createElement("div");
  ov.className = "rdp-closed";
  ov.innerHTML =
    `<div class="rdp-closed-box"><p>${t("rdp-connexion-fermee")}</p>` +
    `<pre class="rdp-closed-diag" hidden></pre>` +
    `<div class="rdp-closed-actions">` +
    `<button type="button" class="btn-primary" data-act="reconnect">${t("rdp-reconnecter")}</button>` +
    `<button type="button" class="btn-ghost" data-act="close">${t("fermer-l-onglet-maj")}</button>` +
    `</div></div>`;
  // « Connexion RDP fermée » sans un mot de plus ne dit pas si le serveur a
  // redémarré, si le réseau a lâché ou si le processus a échoué. Le sidecar
  // écrit ses raisons ; on les montre (avec relances, cf. `lireDiagnosticRdp`).
  void lireDiagnosticRdp(id).then((diag) => {
    const zone = ov.querySelector(".rdp-closed-diag") as HTMLElement | null;
    // L'utilisateur a pu reconnecter ou fermer l'onglet pendant les relances :
    // ne rien écrire dans une incrustation déjà retirée du document.
    if (!zone || !zone.isConnected || !diag) return;
    zone.textContent = diag.split("\n").slice(-4).join("\n");
    zone.hidden = false;
  }).catch(() => { /* pas de diagnostic : l'incrustation reste sobre */ });
  ov.querySelector('[data-act="reconnect"]')!.addEventListener("click", () => {
    const t = s.target;
    closeRdp(id);
    if (t) void openRdp(t);
  });
  ov.querySelector('[data-act="close"]')!.addEventListener("click", () => closeRdp(id));
  wrap.appendChild(ov);
}

/** Connexion à un bureau RDP enregistré (mot de passe du trousseau, sinon demandé). */
export async function connectRdpSaved(h: RdpHostT) {
  // On demande au cœur s'il connaît ce compte, sans jamais rapatrier le secret :
  // un mot de passe vide indique à `rdp_open` de le lire lui-même dans le
  // trousseau. Il ne traverse donc pas l'IPC et ne séjourne pas dans le tas de
  // la webview pour toute la durée de l'onglet.
  const protocole = protocoleDe(h);
  const connu = await invoke<boolean>("rdp_password_known", { host: h.host, port: h.port, user: h.user, protocole }).catch(() => false);
  let pw = "";
  if (!connu) {
    const rep = await askPassword(`${h.user ? `${h.user}@` : ""}${h.host}:${h.port}`);
    if (!rep) return;
    pw = rep.password;
    if (rep.remember && pw) {
      const memorise = await invoke("rdp_password_save", { host: h.host, port: h.port, user: h.user, password: pw, protocole })
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
  await openRdp({
    host: h.host, port: h.port, user: h.user, password: pw,
    hostId: h.id, name: h.name,
    // Choix déjà donné pour ce bureau : on ne le redemande pas à chaque fois.
    sansNla: h.sans_nla === true,
    vnc: protocole === "vnc",
    partage: h.partage,
  });
}

export function openRdpMenu(h: RdpHostT, e: MouseEvent) {
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
  const h = state.rdpHosts.find((x) => x.id === id);
  if (!act || !h) return;
  if (act === "connect") void connectRdpSaved(h);
  else if (act === "edit") openEditRdp(h);
  else if (act === "move") openMoveModal("rdp", h.id);
  else if (act === "forget") {
    // cf. le volet SSH : une action muette ne se distingue pas d'un clic raté.
    await invoke("rdp_password_forget", { host: h.host, port: h.port, user: h.user, protocole: protocoleDe(h) })
      .then(() => notify(t("hote-mdp-oublie", { alias: h.name }), "succes"))
      .catch((err) => notifyErreur(t("hote-mdp-non-oublie", { e: String(err) })));
  } else if (act === "delete") {
    if (!(await askConfirm(t("rdp-supprimer-question", { nom: h.name })))) return;
    await invoke("rdp_host_delete", { id: h.id }).catch((err) => notifyErreur(t("suppression-impossible", { e: String(err) })));
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
  f.dataset.oldProto = protocoleDe(h);
  ($("re-proto") as HTMLSelectElement).value = protocoleDe(h);
  syncProtoEdition();
  ($("re-id") as HTMLInputElement).value = h.id;
  ($("re-name") as HTMLInputElement).value = h.name;
  ($("re-addr") as HTMLInputElement).value = h.host;
  ($("re-port") as HTMLInputElement).value = String(h.port);
  ($("re-user") as HTMLInputElement).value = h.user;
  ($("re-password") as HTMLInputElement).value = "";
  ($("re-partage") as HTMLInputElement).value = h.partage ?? "";
  ($("rdp-edit-form") as HTMLFormElement).dataset.folder = h.folder ?? "";
  $("rdp-edit-modal").classList.add("open");
  setTimeout(() => ($("re-name") as HTMLInputElement).focus(), 30);
}

export function closeEditRdp() {
  $("rdp-edit-modal").classList.remove("open");
}

/** Le formulaire suit le protocole choisi : port par défaut, avertissement VNC. */
function syncProtoEdition() {
  const vnc = ($("re-proto") as HTMLSelectElement).value === "vnc";
  $("re-vnc-hint").hidden = !vnc;
  // Pas de lecteur en VNC : le RFB n'a pas de canal pour ça.
  $("re-partage-row").hidden = vnc;
  ($("re-port") as HTMLInputElement).placeholder = vnc ? "5900" : "3389";
}
$("re-proto").addEventListener("change", syncProtoEdition);

/** Sélecteur de dossier du système pour le lecteur partagé ; annulé, le champ ne bouge pas. */
export async function choisirDossierPartage(champ: HTMLInputElement) {
  const choix = await openDialog({ directory: true, multiple: false, defaultPath: champ.value || undefined }).catch(() => null);
  if (typeof choix === "string" && choix) champ.value = choix;
}
$("re-partage-choisir").addEventListener("click", () => void choisirDossierPartage($("re-partage") as HTMLInputElement));

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
  const protocole = ($("re-proto") as HTMLSelectElement).value === "vnc" ? "vnc" : "rdp";
  const portRaw = val("re-port");
  const port = portRaw ? Number(portRaw) : (protocole === "vnc" ? 5900 : 3389);
  const pw = ($("re-password") as HTMLInputElement).value;
  // L'authentification VNC classique n'a qu'un mot de passe : l'utilisateur
  // n'y est pas requis.
  if (!name || !host || (!user && protocole !== "vnc")) {
    err.textContent = t(protocole === "vnc" ? "rdp-nom-adresse-requis" : "rdp-nom-adresse-utilisateur-requis");
    err.hidden = false;
    return;
  }
  submit.disabled = true;
  try {
    await invoke("rdp_host_save", { id: val("re-id"), name, host, port, user, width: 0, height: 0, folder: ($("rdp-edit-form") as HTMLFormElement).dataset.folder ?? null, protocole, partage: protocole === "vnc" ? null : (val("re-partage") || null) });
    // Le compte du trousseau dépend de host/port/user et du protocole : si
    // l'un change, on migre (ou remplace) le mot de passe mémorisé vers le
    // nouveau compte.
    const oldHost = f.dataset.oldHost ?? host;
    const oldPort = Number(f.dataset.oldPort ?? String(port));
    const oldUser = f.dataset.oldUser ?? user;
    const oldProtocole = f.dataset.oldProto ?? protocole;
    const accountChanged = oldHost !== host || oldPort !== port || oldUser !== user || oldProtocole !== protocole;
    // Trouvé par l'audit du 7 septembre 2026 : ces trois `.catch(() => {})`
    // avalaient l'échec du trousseau (Secret Service absent, session verrouillée,
    // écriture refusée). La fiche se fermait « bureau enregistré » alors que le
    // mot de passe n'était pas mémorisé, ou restait sous l'ancien compte après un
    // changement d'hôte/port/utilisateur : la connexion suivante le redemandait
    // sans que l'utilisateur sache pourquoi. On notifie désormais chaque échec.
    // La fiche peut rester fermée : le bureau, lui, est bien sauvé.
    if (pw) {
      await invoke("rdp_password_save", { host, port, user, password: pw, protocole })
        .catch((ex) => notifyErreur(t("memorisation-impossible", { e: String(ex) })));
      if (accountChanged) {
        await invoke("rdp_password_forget", { host: oldHost, port: oldPort, user: oldUser, protocole: oldProtocole })
          .catch((ex) => notifyErreur(t("hote-mdp-non-oublie", { e: String(ex) })));
      }
    } else if (accountChanged) {
      // Migration confiée au cœur : le secret n'a aucune raison de faire
      // l'aller-retour par l'interface pour changer de clé de trousseau. En cas
      // d'échec le mot de passe reste sous l'ancien compte — on le dit.
      await invoke("rdp_password_move", { oldHost, oldPort, oldUser, host, port, user, oldProtocole, protocole })
        .catch((ex) => notifyErreur(t("rdp-mdp-non-deplace", { e: String(ex) })));
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

// ----- Glisser-déposer sur le bureau distant -----
//
// Des fichiers déposés depuis le poste sur un bureau actif lui sont offerts
// par le presse-papiers : ils se collent ensuite dans son Explorateur. Le
// panneau SFTP a son propre écouteur, qui ne réagit que sur un onglet SSH.
getCurrentWebview()
  .onDragDropEvent((ev) => {
    if (ev.payload.type !== "drop") return;
    const id = bureauActif();
    if (id === null) return;
    offrirFichiers(id, ev.payload.paths);
  })
  .catch(() => { /* hors Tauri (tests) : pas de glisser-déposer */ });

/** Table minimale code clavier → scancode PC (set 1). Suffisant pour saisir. */
