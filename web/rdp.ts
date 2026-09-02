// Bureaux RDP : sessions (canvas), entrées, presse-papiers, bureaux enregistrés.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { ic } from "./icons";
import { partageClipboard } from "./prefs";
import { rdpScancode, le16, rdpMousePos } from "./filters";
import { $, type RdpHostT, state } from "./etat";
import { askConfirm, askPassword } from "./dialogues";
import { closeAllContextMenus, placerMenu } from "./menu-hote";
import { currentLocks } from "./verrous";
import { focusTab, orderedTabs } from "./raccourcis";
import { loadHosts, renderHosts } from "./main";
import { notify, notifyErreur } from "./notifications";
import { openMoveModal } from "./dossiers";
import { t } from "./i18n";

// ---------- RDP (bureau distant, via le sidecar avash-rdp) ----------


type RdpTarget = { host: string; port: number | null; user: string; password: string; width?: number; height?: number; hostId?: string; name?: string; sansNla?: boolean };

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
// Le presse-papiers local n'est PAS poussé au simple retour de la fenêtre : cela
// envoyait son contenu — souvent un mot de passe fraîchement copié — à tout
// serveur RDP ouvert, sans le moindre geste de l'utilisateur, et à chaque
// bascule de fenêtre. Il ne part plus que sur un collage explicite (Ctrl+V) ou
// quand le serveur le réclame, dans l'onglet actif.
export const rdpSessions = new Map<number, { canvas: HTMLCanvasElement; tab: HTMLElement; ws: WebSocket | null; ro?: ResizeObserver; detachRect?: () => void; hostId?: string; syncSize?: () => void; target?: RdpTarget }>();

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
  tab.querySelector(".label")!.textContent = cible.name ?? `${cible.user}@${cible.host}`;
  tab.querySelector(".close")!.innerHTML = ic("x");
  tabs.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
  tabs.appendChild(tab);

  // Résolution = taille de la zone disponible d'Avash (adaptatif), sauf si
  // une taille précise est imposée. RDP : largeur paire, bornes 200..8192.
  const area = $("terminal").getBoundingClientRect();
  const even = (n: number) => n - (n % 2);
  // Mutables : au redimensionnement natif, le serveur renvoie la vraie taille
  // (message CONNECTED) et on les remet à jour — le mappage souris suit.
  let rdpW = Math.max(200, Math.min(8192, even(Math.round(cible.width || area.width || 1280))));
  let rdpH = Math.max(200, Math.min(8192, Math.round(cible.height || area.height || 800)));

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
  wrap.appendChild(canvas);
  wrap.appendChild(hud);
  $("terminal").appendChild(wrap);
  const ctx = canvas.getContext("2d")!;
  rdpSessions.set(id, { canvas, tab, ws: null, hostId: cible.hostId, target: cible });
  state.active = id;

  tab.addEventListener("click", () => focusRdp(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => { e.stopPropagation(); closeRdp(id); });

  // Souris/clavier → sidecar via le WebSocket (binaire). Ignore si non prêt.
  const send = (bytes: number[]) => {
    const s = rdpSessions.get(id);
    if (s?.ws && s.ws.readyState === WebSocket.OPEN) s.ws.send(new Uint8Array(bytes));
  };
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
  const detachRect = () => {
    window.removeEventListener("resize", invaliderRect);
    window.removeEventListener("scroll", invaliderRect, true);
  };
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
    invaliderRect(); // l'onglet vient (peut-être) de devenir visible : rect à relire
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
    invaliderRect(); // la zone a changé de taille : le rect mémorisé est périmé
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(sendResize, 400);
  });
  ro.observe($("terminal"));
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
      sansNla: cible.sansNla === true || sansNlaAccepte.has(`${cible.host}:${cible.port ?? 3389}`),
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
  state.active = id;
  for (const [sid, s] of rdpSessions) {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    (s.canvas.parentElement as HTMLElement).style.display = active ? "flex" : "none";
    marquerVisibilite(s, active);
    if (active) {
      donnerLeFocusAuBureau(s.canvas);
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

export function closeRdp(id: number) {
  const s = rdpSessions.get(id);
  if (!s) return;
  if (document.body.classList.contains("rdp-full")) {
    document.body.classList.remove("rdp-full");
    getCurrentWindow().setFullscreen(false).catch(() => {});
  }
  s.ro?.disconnect();
  s.detachRect?.();
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
    `<div class="rdp-closed-box"><p>${t("rdp-connexion-fermee")}</p>` +
    `<pre class="rdp-closed-diag" hidden></pre>` +
    `<div class="rdp-closed-actions">` +
    `<button type="button" class="btn-primary" data-act="reconnect">${t("rdp-reconnecter")}</button>` +
    `<button type="button" class="btn-ghost" data-act="close">${t("fermer-l-onglet-maj")}</button>` +
    `</div></div>`;
  // « Connexion RDP fermée » sans un mot de plus ne dit pas si le serveur a
  // redémarré, si le réseau a lâché ou si le processus a échoué. Le sidecar
  // écrit ses raisons ; on les montre.
  void invoke<string>("rdp_diagnostic", { id }).then((diag) => {
    const zone = ov.querySelector(".rdp-closed-diag") as HTMLElement | null;
    if (!zone || !diag.trim()) return;
    zone.textContent = diag.trim().split("\n").slice(-4).join("\n");
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
  await openRdp({
    host: h.host, port: h.port, user: h.user, password: pw,
    hostId: h.id, name: h.name,
    // Choix déjà donné pour ce bureau : on ne le redemande pas à chaque fois.
    sansNla: h.sans_nla === true,
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
    await invoke("rdp_password_forget", { host: h.host, port: h.port, user: h.user })
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

export function closeEditRdp() {
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
    err.textContent = t("rdp-nom-adresse-utilisateur-requis");
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
