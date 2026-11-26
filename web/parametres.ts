// Fenêtre des paramètres : les réglages d'Avash réunis, avec de vraies cases.
//
// Demandée par le mainteneur le 26 novembre 2026 : « un vrai bouton pour les
// paramètres plutôt que de devoir taper Ctrl+K ». Les réglages n'existaient
// qu'en bascules de la palette, dispersées (et la durée du journal dans la
// fenêtre du journal) ; la palette les garde, la fenêtre les montre tous, par
// section. Chaque contrôle fait ce que fait sa bascule de palette, effets
// compris (les bureaux ouverts suivent le partage du presse-papiers, la barre
// latérale se retraduit), sans les notifications : la case dit l'état.

import { $, FONT_MAX, FONT_MIN, state } from "./etat";
import { langue, setLangue, type Langue } from "./i18n";
import {
  confirmerFermetureOnglet,
  ctrlCopierColler,
  dureeJournal,
  journalGlobal,
  partageClipboard,
  setConfirmerFermetureOnglet,
  setCtrlCopierColler,
  setDureeJournal,
  setPartageClipboard,
  setSonBureau,
  setSondeAuDemarrage,
  sonBureau,
  sondeAuDemarrage,
} from "./prefs";
import { annoncerPartageClip, rdpSessions } from "./rdp";
import { basculerJournal, journalOpen, majReperesJournal, purgerJournal } from "./journal-commandes";
import { choisirTheme, themeChoisi, type ThemePref } from "./theme";
import { setFontSize } from "./terminal-outils";
import { estWindows } from "./vnc-clavier";

const modal = () => $("parametres-modal");
const caseA = (id: string) => $(id) as HTMLInputElement;
const choix = (id: string) => $(id) as HTMLSelectElement;

/** Ce que la fenêtre ne peut faire seule : retracer la barre latérale, qui vit
 *  dans le module d'entrée. */
let apresLangue: () => void = () => {};

/** Remet chaque contrôle sur l'état du réglage : à l'ouverture, et quand un
 *  réglage change par ailleurs (palette, fenêtre du journal). */
function rendre(): void {
  choix("param-langue").value = langue();
  choix("param-theme").value = themeChoisi();
  caseA("param-fermeture").checked = confirmerFermetureOnglet();
  caseA("param-sante").checked = sondeAuDemarrage();
  ($("param-taille") as HTMLInputElement).value = String(state.terminalFontSize);
  caseA("param-ctrl").checked = ctrlCopierColler(estWindows());
  caseA("param-clip").checked = partageClipboard();
  caseA("param-son").checked = sonBureau();
  caseA("param-journal").checked = journalGlobal();
  // La durée choisie est toujours une option de la liste : une valeur hors
  // liste (ancienne) ajoute la sienne, comme dans la fenêtre du journal.
  const duree = String(dureeJournal());
  const liste = choix("param-journal-duree");
  if (![...liste.options].some((o) => o.value === duree)) liste.add(new Option(duree, duree));
  liste.value = duree;
}

export function parametresOpen(): void {
  rendre();
  modal().classList.add("open");
  choix("param-langue").focus();
}

function parametresClose(): void {
  modal().classList.remove("open");
}

/** Pose les écouteurs. `apres` reçoit ce que le module d'entrée doit refaire
 *  quand la langue change. */
export function brancherParametres(apres: { langue: () => void }): void {
  apresLangue = apres.langue;
  $("parametres-btn").addEventListener("click", parametresOpen);
  $("parametres-close").addEventListener("click", parametresClose);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && modal().classList.contains("open")) parametresClose();
  });

  choix("param-langue").addEventListener("change", () => {
    setLangue(choix("param-langue").value as Langue);
    apresLangue();
    majReperesJournal();
  });
  choix("param-theme").addEventListener("change", () => choisirTheme(choix("param-theme").value as ThemePref));
  caseA("param-fermeture").addEventListener("change", () => setConfirmerFermetureOnglet(caseA("param-fermeture").checked));
  caseA("param-sante").addEventListener("change", () => setSondeAuDemarrage(caseA("param-sante").checked));
  const taille = $("param-taille") as HTMLInputElement;
  taille.min = String(FONT_MIN);
  taille.max = String(FONT_MAX);
  taille.addEventListener("change", () => {
    const n = Number(taille.value);
    if (Number.isFinite(n)) setFontSize(Math.round(n));
    taille.value = String(state.terminalFontSize);
  });
  caseA("param-ctrl").addEventListener("change", () => setCtrlCopierColler(caseA("param-ctrl").checked));
  caseA("param-clip").addEventListener("change", () => {
    setPartageClipboard(caseA("param-clip").checked);
    // Les bureaux ouverts suivent, comme depuis la palette.
    for (const s of rdpSessions.values()) if (s.ws) annoncerPartageClip(s.ws);
  });
  caseA("param-son").addEventListener("change", () => setSonBureau(caseA("param-son").checked));
  caseA("param-journal").addEventListener("change", () => {
    if (caseA("param-journal").checked !== journalGlobal()) basculerJournal();
    caseA("param-journal").checked = journalGlobal();
  });
  choix("param-journal-duree").addEventListener("change", () => {
    setDureeJournal(Number(choix("param-journal-duree").value));
    purgerJournal();
  });
  $("param-journal-ouvrir").addEventListener("click", () => {
    parametresClose();
    void journalOpen();
  });
}
