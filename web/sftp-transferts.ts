// File des transferts SFTP : états et décisions, sans DOM.
//
// Audit du 12 septembre 2026 (C-front-5 et couverture.md, section 5, point 3) :
// la file vivait dans sftp.ts, mêlée à son rendu, et ne se testait qu'à travers
// la page. Les décisions (qui part, comment une issue se classe, ce que la ligne
// affiche, quand le bouton « Annuler » a un sens) sont ici, testées sans
// document ; sftp.ts ne garde que le câblage et le rendu de chaque ligne.

import { humanSize } from "./filters";
import { langue, t } from "./i18n";

export type EtatTransfert = "attente" | "en-cours" | "fini" | "erreur" | "annule";
export type SorteTransfert = "download" | "upload" | "copie";

export type Transfert<S = unknown> = {
  id: number;
  kind: SorteTransfert;
  nom: string;
  fichier: string;
  fait: number;
  total: number;
  termines: number;
  nombre: number;
  etat: EtatTransfert;
  vitesse: number;
  message: string;
  lancer: () => Promise<string>;
  /** Pour la vitesse : dernier point mesuré. */
  dernierT: number;
  dernierFait: number;
  /** Session d'origine, pour rafraîchir sa liste à la fin. */
  session: S;
  /** Copie directe (scp menée par l'hôte source) : le cœur n'inscrit aucun
   *  drapeau et n'émet aucune progression pour elle. Trouvé par l'audit du
   *  7 septembre 2026 : sans ce repère la ligne offrait un bouton « Annuler »
   *  sans effet et restait bloquée à « 0 o ». */
  direct: boolean;
};

/** Événement `sftp-progress` du cœur, rapporté à sa ligne par `transfert`. */
export type Progression = { transfert: number; fichier: string; done: number; total: number; termines: number; nombre: number };

/** Transferts menés de front ; les suivants attendent leur tour. */
export const PARALLELE = 3;

/** Un transfert neuf, en attente. */
export function nouveauTransfert<S>(
  id: number, kind: SorteTransfert, nom: string, session: S, lancer: (id: number) => Promise<string>, direct = false,
): Transfert<S> {
  return {
    id, kind, nom, fichier: "", fait: 0, total: 0, termines: 0, nombre: 1,
    etat: "attente", vitesse: 0, message: "", lancer: () => lancer(id),
    dernierT: 0, dernierFait: 0, session, direct,
  };
}

/** Les transferts en attente qui peuvent partir maintenant, dans l'ordre de la
 *  file, sans dépasser `parallele` transferts en cours. */
export function aLancer<T extends Pick<Transfert, "etat">>(file: readonly T[], parallele = PARALLELE): T[] {
  let enCours = file.filter((x) => x.etat === "en-cours").length;
  const partants: T[] = [];
  for (const x of file) {
    if (enCours >= parallele) break;
    if (x.etat !== "attente") continue;
    partants.push(x);
    enCours++;
  }
  return partants;
}

/** Classe l'issue d'un transfert. Le cœur dit « Transfert annulé » quand
 *  l'annulation a pris : la ligne passe « annulé » (reprise possible), pas
 *  « erreur ». Un succès remplit la barre même si aucun événement de
 *  progression n'est arrivé (copie directe, fichier vide). */
export function conclure(x: Transfert, issue: { ok: true; message: string } | { ok: false; erreur: string }): void {
  if (issue.ok) {
    x.etat = "fini";
    x.message = issue.message;
    x.fait = x.total || x.fait;
  } else {
    x.etat = issue.erreur.includes("Transfert annulé") ? "annule" : "erreur";
    x.message = issue.erreur;
  }
}

/** Reporte un événement de progression sur son transfert. La vitesse est une
 *  moyenne mobile recalculée au plus toutes les demi-secondes, pour que le
 *  chiffre ne tremble pas à la cadence des événements (80 ms). */
export function appliquerProgression(x: Transfert, p: Progression, maintenant: number): void {
  const dt = (maintenant - x.dernierT) / 1000;
  if (dt >= 0.5) {
    const instant = (p.done - x.dernierFait) / dt;
    x.vitesse = x.vitesse > 0 ? x.vitesse * 0.6 + instant * 0.4 : instant;
    x.dernierT = maintenant;
    x.dernierFait = p.done;
  }
  x.fichier = p.nombre > 1 ? p.fichier : "";
  x.fait = p.done;
  x.total = p.total;
  x.termines = p.termines;
  x.nombre = p.nombre;
}

/** Largeur de la barre, en pour cent. */
export function pourcentage(x: Pick<Transfert, "etat" | "fait" | "total">): number {
  if (x.etat === "fini") return 100;
  return x.total ? Math.round((x.fait / x.total) * 100) : 0;
}

/** Le texte de détail d'une ligne, dans la langue courante. */
export function detailTransfert(x: Transfert): string {
  if (x.etat === "en-cours" && x.direct && x.kind === "copie") {
    // Copie directe : aucun événement de progression (scp chez la source),
    // « 0 o » laissait croire à un transfert figé : on dit ce qui se passe.
    return t("sftp-copie-directe-en-cours");
  }
  if (x.etat === "en-cours") {
    const l = langue();
    const vit = x.vitesse > 0 ? ` · ${humanSize(Math.round(x.vitesse), l)}/s` : "";
    return x.nombre > 1
      ? `${t("sftp-elements-faits", { fait: humanSize(x.fait, l), total: humanSize(x.total, l), termines: x.termines, nombre: x.nombre })}${vit}${x.fichier ? ` · ${x.fichier}` : ""}`
      : `${humanSize(x.fait, l)}${x.total ? ` / ${humanSize(x.total, l)}` : ""}${vit}`;
  }
  if (x.etat === "attente") return "…";
  if (x.etat === "fini") return t("sftp-transfert-fini");
  if (x.etat === "annule") return t("sftp-transfert-annule");
  return x.message;
}

/** Le bouton « Annuler » n'a de sens que si l'annulation peut aboutir.
 *
 *  Extrait pour le test (audit du 7 septembre 2026). Une copie directe *en
 *  cours* est menée par scp chez l'hôte source : le cœur ne lève aucun drapeau,
 *  sftp_annuler rend false et rien ne s'arrête ; proposer le bouton mentait.
 *  Tant qu'elle *attend* son tour, en revanche, l'annulation est purement
 *  locale (le scp n'a pas démarré) et le bouton reste légitime. */
export function boutonAnnulerVisible(etat: EtatTransfert, kind: SorteTransfert, direct: boolean): boolean {
  if (etat === "attente") return true;
  if (etat === "en-cours") return !(direct && kind === "copie");
  return false;
}

/** Une ligne terminée (réussie, en erreur ou annulée) s'efface d'un clic ; une
 *  ligne vivante non : une copie directe en cours n'a ni bouton ni effacement,
 *  le scp tourne toujours chez la source. */
export function effacableAuClic(etat: EtatTransfert): boolean {
  return etat === "fini" || etat === "erreur" || etat === "annule";
}
