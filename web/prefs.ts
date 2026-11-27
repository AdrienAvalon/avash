// Réglages retenus d'un lancement à l'autre.

import { FONT_MAX, FONT_MIN } from "./etat";
//
// Isolés du module d'entrée pour être exerçables par les tests : ils décident de
// ce qui sort de la machine, ce qui les rend trop importants pour n'être
// couverts que de bout en bout.

/** Le presse-papiers est-il échangé avec les bureaux distants ?
 *
 *  Le réglage vaut dans les **deux** sens. Sortant : confier le contenu du
 *  presse-papiers — souvent un mot de passe qu'on vient de copier — à un
 *  serveur qui peut le réclamer dès qu'on le lui annonce. Entrant : laisser un
 *  bureau distant remplacer le presse-papiers du poste, ce dont un serveur
 *  hostile se sert pour faire coller autre chose que ce qu'on croit avoir
 *  copié. C'est le comportement attendu d'un client RDP, donc le défaut, mais
 *  il doit rester révocable : la bascule est offerte à la palette (Ctrl+K).
 *  L'absence de réglage vaut « partagé ». */
export const CLIP_KEY = "avash.rdp.clipboard";

export function partageClipboard(): boolean {
  // Un navigateur peut refuser l'accès au stockage (mode privé strict) : on
  // retombe alors sur le défaut plutôt que de faire échouer l'appelant.
  try {
    return localStorage.getItem(CLIP_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setPartageClipboard(actif: boolean): void {
  try {
    localStorage.setItem(CLIP_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}

/** Le son des bureaux distants : joué par défaut, coupé dans la palette. Le
 *  réglage vaut pour les prochaines connexions (le canal se négocie à
 *  l'ouverture). */
const SON_KEY = "avash.rdp.son";

export function sonBureau(): boolean {
  try {
    return localStorage.getItem(SON_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setSonBureau(actif: boolean): void {
  try {
    localStorage.setItem(SON_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}

/** Sonder la santé des hôtes au démarrage : coupé par défaut — une sonde
 *  ouvre une connexion vers chaque serveur, ce n'est pas anodin partout. */
export const SANTE_DEMARRAGE_KEY = "avash.sante.demarrage";

export function sondeAuDemarrage(): boolean {
  try {
    return localStorage.getItem(SANTE_DEMARRAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function setSondeAuDemarrage(actif: boolean): void {
  try {
    localStorage.setItem(SANTE_DEMARRAGE_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}

/** Demander avant de fermer un onglet vivant (session SSH ou série ouverte,
 *  bureau distant connecté). Audit du 12 septembre 2026 (C-front-6) : toutes les
 *  actions destructrices passaient par une confirmation, sauf celle-ci, pourtant
 *  irréversible pour ce qui tourne dans la session ; une croix de 18 px visée un
 *  peu trop à droite, ou Ctrl+W par réflexe readline, tuait une migration en
 *  cours. Demandé par défaut ; « Ne plus demander » se règle à la palette. */
export const FERMETURE_KEY = "avash.onglets.confirmer-fermeture";

export function confirmerFermetureOnglet(): boolean {
  try {
    return localStorage.getItem(FERMETURE_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setConfirmerFermetureOnglet(actif: boolean): void {
  try {
    localStorage.setItem(FERMETURE_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}

/** Journal des commandes passées en SSH (voir `journal-capteur.ts`) : coupé
 *  par défaut. Il note sur le disque ce qu'on fait sur des serveurs, ce que
 *  personne ne doit découvrir après coup ; il s'allume à la palette. */
export const JOURNAL_KEY = "avash.journal.commandes";
/** Exceptions par connexion, `{ "alias": true | false }` : une connexion
 *  journalisée alors que le journal est coupé, ou l'inverse. */
export const JOURNAL_HOTES_KEY = "avash.journal.commandes.hotes";
/** Durée de conservation du journal, en jours. */
export const JOURNAL_JOURS_KEY = "avash.journal.commandes.jours";
export const JOURNAL_JOURS_DEFAUT = 90;

export function journalGlobal(): boolean {
  try {
    return localStorage.getItem(JOURNAL_KEY) === "1";
  } catch {
    return false;
  }
}

export function setJournalGlobal(actif: boolean): void {
  try {
    localStorage.setItem(JOURNAL_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}

function exceptionsJournal(): Record<string, boolean> {
  try {
    const brut: unknown = JSON.parse(localStorage.getItem(JOURNAL_HOTES_KEY) ?? "{}");
    if (!brut || typeof brut !== "object" || Array.isArray(brut)) return {};
    return Object.fromEntries(Object.entries(brut).filter(([, v]) => typeof v === "boolean")) as Record<string, boolean>;
  } catch {
    return {};
  }
}

/** Le journal vaut-il pour cette connexion ? Son exception, sinon le réglage. */
export function journalActifPour(alias: string): boolean {
  const e = exceptionsJournal();
  return Object.hasOwn(e, alias) ? e[alias] : journalGlobal();
}

/** L'exception de cette connexion, ou `null` quand elle suit le réglage. */
export function exceptionJournal(alias: string): boolean | null {
  const e = exceptionsJournal();
  return Object.hasOwn(e, alias) ? e[alias] : null;
}

/** Inverse le journal pour cette connexion. Une exception qui revient au
 *  réglage général disparaît : elle suivra ses changements. */
export function basculerJournalPour(alias: string): void {
  const e = exceptionsJournal();
  const voulu = !journalActifPour(alias);
  if (voulu === journalGlobal()) delete e[alias];
  else e[alias] = voulu;
  try {
    localStorage.setItem(JOURNAL_HOTES_KEY, JSON.stringify(e));
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}

export function dureeJournal(): number {
  try {
    const n = Number.parseInt(localStorage.getItem(JOURNAL_JOURS_KEY) ?? "", 10);
    return Number.isInteger(n) && n >= 1 && n <= 3650 ? n : JOURNAL_JOURS_DEFAUT;
  } catch {
    return JOURNAL_JOURS_DEFAUT;
  }
}

export function setDureeJournal(jours: number): void {
  try {
    localStorage.setItem(JOURNAL_JOURS_KEY, String(Math.min(3650, Math.max(1, Math.round(jours)))));
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}

/** Ctrl+C et Ctrl+V copient et collent dans un terminal, comme dans Windows
 *  Terminal (voir `gesteCopierCollerCtrl`, filters.ts). Sans réglage : oui
 *  sous Windows, où c'est l'usage, non ailleurs, où Ctrl+V sert au shell et à
 *  vim. Les paramètres le changent dans les deux sens. */
const CTRL_COPIER_COLLER_KEY = "avash.terminal.ctrl-copier-coller";

export function ctrlCopierColler(surWindows: boolean): boolean {
  try {
    const v = localStorage.getItem(CTRL_COPIER_COLLER_KEY);
    if (v === "1") return true;
    if (v === "0") return false;
  } catch {
    /* stockage indisponible : le défaut du système */
  }
  return surWindows;
}

export function setCtrlCopierColler(actif: boolean): void {
  try {
    localStorage.setItem(CTRL_COPIER_COLLER_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}

/** La taille du texte des terminaux, en pixels. Elle ne survivait pas à un
 *  redémarrage : Ctrl+ et Ctrl- ne valaient que pour la session. Bornée ici
 *  aussi, pour qu'une valeur abîmée dans le stockage ne rende pas les
 *  terminaux illisibles. */
const TAILLE_TEXTE_KEY = "avash.terminal.taille";
export const TAILLE_TEXTE_DEFAUT = 14;

export function tailleTexte(): number {
  try {
    const n = Number(localStorage.getItem(TAILLE_TEXTE_KEY));
    if (Number.isInteger(n) && n >= FONT_MIN && n <= FONT_MAX) return n;
  } catch {
    /* stockage indisponible */
  }
  return TAILLE_TEXTE_DEFAUT;
}

export function setTailleTexte(px: number): void {
  try {
    localStorage.setItem(TAILLE_TEXTE_KEY, String(px));
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}

/** Lignes d'historique gardées par chaque terminal.
 *
 *  Audit du 27 novembre 2026 (ME-2), choix du mainteneur : 10 000 lignes
 *  étaient figées. xterm garde douze octets par cellule : un historique plein
 *  pèse 13 Mo à 80 colonnes, 26 Mo à 200, par onglet, jusqu'à sa fermeture.
 *  Le réglage vit dans la fenêtre Paramètres, quatre valeurs, 10 000 par
 *  défaut (Windows Terminal en garde 9 001, VTE 10 000). Une valeur hors
 *  liste dans le stockage retombe sur le défaut. */
const HISTORIQUE_KEY = "avash.terminal.historique";
export const HISTORIQUE_CHOIX = [1000, 5000, 10000, 50000] as const;
export const HISTORIQUE_DEFAUT = 10000;

export function historiqueTerminal(): number {
  try {
    const n = Number(localStorage.getItem(HISTORIQUE_KEY));
    if ((HISTORIQUE_CHOIX as readonly number[]).includes(n)) return n;
  } catch {
    /* stockage indisponible */
  }
  return HISTORIQUE_DEFAUT;
}

export function setHistoriqueTerminal(lignes: number): void {
  if (!(HISTORIQUE_CHOIX as readonly number[]).includes(lignes)) return;
  try {
    localStorage.setItem(HISTORIQUE_KEY, String(lignes));
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}
