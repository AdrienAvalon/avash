// Journal des commandes passées en SSH : le capteur de chaque onglet, le
// repère d'onglet et la fenêtre du journal (recherche, export, conservation).
// Voir `journal-capteur.ts` pour ce qui est noté et pourquoi.

import { invoke } from "@tauri-apps/api/core";
import type { Terminal } from "@xterm/xterm";
import { $, state, type Session } from "./etat";
import { langue, t } from "./i18n";
import { CapteurCommandes } from "./journal-capteur";
import { notify, notifyErreur } from "./notifications";
import { dureeJournal, journalActifPour, journalGlobal, setDureeJournal, setJournalGlobal } from "./prefs";

type Entree = { t: number; s: number; session: string; cible: string; commande: string; code?: number };

/** Au-delà, la fenêtre n'affiche plus : elle invite à filtrer. */
const AFFICHAGE_MAX = 500;

/** Le journal vaut-il pour cet onglet ? Jamais pour une console série. */
export function journalActifPourOnglet(s: Session): boolean {
  return !s.serie && !s.closed && s.etat === "live" && journalActifPour(s.alias);
}

// Une écriture refusée (disque plein, dossier en lecture seule) se dit une
// fois par lancement, pas à chaque commande.
let echecSignale = false;
function signalerEchec(e: unknown) {
  if (echecSignale) return;
  echecSignale = true;
  notifyErreur(t("journal-ecriture-impossible", { e: String(e) }));
}

/** Pose le capteur de commandes sur le terminal d'un onglet. */
export function brancherJournal(s: Session, term: Terminal): CapteurCommandes | undefined {
  // Un journal en panne ne doit jamais empêcher d'ouvrir une session : on le
  // dit, et l'onglet s'ouvre sans lui.
  try {
    return creerCapteur(s, term);
  } catch (e) {
    signalerEchec(e);
    return undefined;
  }
}

function creerCapteur(s: Session, term: Terminal): CapteurCommandes {
  return new CapteurCommandes(
    term,
    (commande) => {
      invoke("journal_noter", { id: s.id, commande }).catch(signalerEchec);
    },
    (code) => {
      invoke("journal_noter_code", { id: s.id, code }).catch(signalerEchec);
    },
    () => journalActifPourOnglet(s),
  );
}

/** Les compteurs de diagnostic des capteurs ouverts (voir
 *  `CapteurCommandes.diagnostic`) : des nombres et des formes de frappes, rien
 *  de ce qui a été tapé. Lus par la suite bout en bout quand le journal reste
 *  vide. */
declare global {
  interface Window {
    __journalDiagnostic?: () => unknown[];
  }
}
window.__journalDiagnostic = () => [...state.sessions.values()].map((s) => s.capteur?.diagnostic ?? null);

/** Le repère de l'onglet suit le réglage : à appeler quand il change, ou quand
 *  l'onglet devient vivant. */
export function majReperesJournal(): void {
  for (const s of state.sessions.values()) {
    const actif = journalActifPourOnglet(s);
    s.tab.classList.toggle("journal", actif);
    let repere = s.tab.querySelector<HTMLElement>(".journal-repere");
    if (actif && !repere) {
      repere = document.createElement("span");
      repere.className = "journal-repere";
      repere.setAttribute("role", "img");
      s.tab.querySelector(".label")?.after(repere);
    }
    if (repere) {
      repere.hidden = !actif;
      repere.title = t("onglet-journal");
      repere.setAttribute("aria-label", t("onglet-journal"));
    }
  }
}

/** Efface les jours trop vieux : au démarrage, et quand la durée change. */
export function purgerJournal(): void {
  invoke("journal_purger", { jours: dureeJournal() }).catch(() => {});
}

// ---------- Fenêtre du journal ----------

const modal = () => $("journal-modal");
let entrees: Entree[] = [];

function quand(e: Entree): string {
  return new Date(e.t * 1000).toLocaleString(langue() === "en" ? "en-GB" : "fr-FR");
}

/** Les commandes qui contiennent chacun des mots du filtre, où qu'ils soient
 *  (connexion, cible, date, commande), sans tenir compte de la casse. */
export function filtrerJournal<T extends { session: string; cible: string; commande: string }>(
  liste: T[],
  filtre: string,
  date: (e: T) => string,
): T[] {
  const mots = filtre.toLowerCase().split(/\s+/).filter(Boolean);
  if (mots.length === 0) return liste;
  return liste.filter((e) => {
    const texte = `${e.session} ${e.cible} ${date(e)} ${e.commande}`.toLowerCase();
    return mots.every((m) => texte.includes(m));
  });
}

function rendre() {
  const zone = $("journal-liste");
  zone.replaceChildren();
  const filtre = ($("journal-filtre") as HTMLInputElement).value;
  const retenues = filtrerJournal(entrees, filtre, quand);
  if (retenues.length === 0) {
    const vide = document.createElement("div");
    vide.className = "empty";
    vide.textContent = t(entrees.length === 0 ? "journal-aucun" : "journal-aucun-filtre");
    zone.appendChild(vide);
    return;
  }
  for (const e of retenues.slice(0, AFFICHAGE_MAX)) {
    // textContent partout : une commande est du texte venu d'un serveur.
    const row = document.createElement("div");
    row.className = "journal-row";
    const tete = document.createElement("div");
    tete.className = "journal-tete";
    const date = document.createElement("span");
    date.className = "quand";
    date.textContent = quand(e);
    const ou = document.createElement("span");
    ou.className = "ou";
    ou.textContent = `${e.session} · ${e.cible}`;
    tete.append(date, ou);
    if (e.code !== undefined) {
      const code = document.createElement("span");
      code.className = e.code === 0 ? "code ok" : "code echec";
      code.textContent = t("journal-code", { code: String(e.code) });
      tete.append(code);
    }
    const cmd = document.createElement("code");
    cmd.className = "commande";
    cmd.textContent = e.commande;
    row.append(tete, cmd);
    zone.appendChild(row);
  }
  if (retenues.length > AFFICHAGE_MAX) {
    const plus = document.createElement("div");
    plus.className = "empty";
    plus.textContent = t("journal-plus", { n: String(retenues.length - AFFICHAGE_MAX) });
    zone.appendChild(plus);
  }
}

function rendreEtat() {
  const actif = journalGlobal();
  $("journal-etat").textContent = t(actif ? "journal-etat-actif" : "journal-etat-coupe");
  $("journal-bascule").textContent = t(actif ? "journal-couper" : "journal-activer");
  const choix = $("journal-duree") as HTMLSelectElement;
  const duree = String(dureeJournal());
  if (![...choix.options].some((o) => o.value === duree)) {
    choix.add(new Option(t("journal-duree-jours", { n: duree }), duree));
  }
  choix.value = duree;
}

/** Allume ou coupe le journal pour toutes les connexions sans exception. */
export function basculerJournal(): void {
  const actif = !journalGlobal();
  setJournalGlobal(actif);
  majReperesJournal();
  notify(t(actif ? "journal-active" : "journal-coupe"), "succes");
  if (modal().classList.contains("open")) rendreEtat();
}

export async function journalOpen() {
  modal().classList.add("open");
  rendreEtat();
  ($("journal-filtre") as HTMLInputElement).focus();
  try {
    entrees = await invoke<Entree[]>("journal_lister");
  } catch (e) {
    entrees = [];
    notifyErreur(t("journal-lecture-impossible", { e: String(e) }));
  }
  rendre();
}

function journalClose() {
  modal().classList.remove("open");
  entrees = [];
}

$("journal-filtre").addEventListener("input", rendre);
$("journal-close").addEventListener("click", journalClose);
$("journal-bascule").addEventListener("click", basculerJournal);
$("journal-duree").addEventListener("change", () => {
  setDureeJournal(Number(($("journal-duree") as HTMLSelectElement).value));
  purgerJournal();
});
$("journal-dossier").addEventListener("click", () => {
  invoke<string>("journal_ouvrir_dossier").catch((e) => notifyErreur(t("journal-dossier-impossible", { e: String(e) })));
});
$("journal-exporter").addEventListener("click", () => {
  // L'heure locale de l'export : minutes à ajouter au temps universel.
  invoke<string | null>("journal_exporter", { decalageMinutes: -new Date().getTimezoneOffset() }).then(
    (chemin) => { if (chemin) notify(t("journal-exporte", { chemin }), "succes"); },
    (e) => notifyErreur(t("journal-export-impossible", { e: String(e) })),
  );
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && modal().classList.contains("open")) journalClose();
});
