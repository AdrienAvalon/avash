// Notifications non bloquantes.

import { $ } from "./etat";
import { t } from "./i18n";

// ---------- Notifications ----------
//
// Remplace `notifyErreur()`, qui sous WebKitGTK/WRY ne bloque pas et n'affiche pas
// nécessairement quoi que ce soit — la même famille de piège que `confirm()`
// et `prompt()`. Un bandeau n'interrompt rien : l'utilisateur lit l'erreur
// sans perdre ce qu'il était en train de faire, et peut la faire disparaître.

/** Nature du message : change la couleur du liseré et l'insistance annoncée. */
type NatureAvis = "info" | "erreur" | "succes";

/**
 * Affiche un bandeau temporaire en bas à droite.
 *
 * Les erreurs restent plus longtemps et sont annoncées de façon assertive aux
 * lecteurs d'écran : ce sont elles qu'il ne faut pas manquer.
 */
/** Raccourci : tous les anciens `alert()` signalaient un échec. */
export function notifyErreur(message: string): void {
  notify(message, "erreur");
}

export function notify(message: string, nature: NatureAvis = "info"): void {
  const zone = $("toasts");
  zone.setAttribute("aria-live", nature === "erreur" ? "assertive" : "polite");
  const el = document.createElement("div");
  el.className = `toast ${nature}`;
  el.setAttribute("role", nature === "erreur" ? "alert" : "status");
  const titre = document.createElement("span");
  titre.className = "titre";
  titre.textContent = nature === "erreur" ? t("notif-echec") : nature === "succes" ? t("notif-fait") : t("notif-information");
  const corps = document.createElement("span");
  corps.textContent = message; // textContent : le message peut venir d'un serveur
  const aide = document.createElement("span");
  aide.className = "fermer";
  aide.textContent = t("notif-cliquer-fermer");
  el.append(titre, corps, aide);

  const retirer = () => {
    el.remove();
    if (zone.children.length === 0) zone.setAttribute("aria-live", "polite");
  };
  el.addEventListener("click", retirer);
  zone.appendChild(el);
  window.setTimeout(retirer, nature === "erreur" ? 9000 : 4500);
}
