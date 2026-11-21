// Barre d'état de sécurité d'une session SSH (refonte de novembre 2026).
//
// Jusque-là, rien dans l'interface ne disait ce qu'une connexion valait : la clé
// d'hôte était vérifiée, les algorithmes négociés, l'authentification choisie,
// mais tout cela restait dans le cœur. Or le moment le plus fragile d'une
// connexion, le premier contact (clé apprise sans que personne ne l'ait
// comparée à une référence), passait exactement comme une reconnexion à un
// hôte connu. La barre montre ce que la poignée de main a établi, et rien
// d'autre : chaque champ vient du cœur (`session_securite`), aucun n'est déduit
// de la configuration.
import { state, type Session } from "./etat";
import { t } from "./i18n";

/** Reflet de `avash::ssh::SecuriteSession` (noms des champs compris). */
export type SecuriteSession = {
  cle_hote: "connue" | "premier-contact" | null;
  algorithme_cle_hote: string;
  empreinte_cle_hote: string;
  echange_de_cles: string;
  chiffrement: string;
  integrite: string | null;
  authentification: "cle" | "agent" | "mot-de-passe" | "clavier-interactif" | null;
};

/** Le bouclier de la barre, construit nœud par nœud : aucun HTML n'est
 *  interprété dans ce module, pas même une constante. */
function bouclier(): SVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const trait = document.createElementNS(NS, "path");
  trait.setAttribute("d", "M12 3 19 6v6c0 5-3 8-7 9-4-1-7-4-7-9V6Z");
  svg.append(trait);
  return svg;
}

function morceau(classe: string, texte: string, titre?: string): HTMLElement {
  const el = document.createElement("span");
  el.className = classe;
  // `textContent`, jamais `innerHTML` : l'empreinte et les noms d'algorithmes
  // viennent de l'échange avec le serveur.
  el.textContent = texte;
  if (titre) el.title = titre;
  return el;
}

/**
 * Remplit la barre pour la session donnée, ou la masque quand il n'y a pas de
 * session SSH à décrire (accueil, bureau distant, console série).
 *
 * La barre reste affichée pendant toute la vie de l'onglet, connexion en cours
 * et session terminée comprises : la faire apparaître à la connexion changerait
 * la hauteur du terminal juste après l'ouverture du shell, donc sa taille en
 * lignes, sous les yeux de l'utilisateur.
 */
export function rendreBarreSecurite(barre: HTMLElement, s: Session | null): void {
  if (!s || s.serie) {
    barre.hidden = true;
    barre.replaceChildren();
    return;
  }
  barre.hidden = false;
  barre.classList.remove("premier-contact");
  const e = s.securite;
  if (s.etat !== "live" || !e) {
    const texte = s.etat === "closed" ? t("session-terminee") : s.etat === "live" ? t("session-connectee") : t("session-connexion-en-cours");
    barre.replaceChildren(morceau("bs-etat", texte));
    return;
  }

  const enfants: HTMLElement[] = [];
  const premier = e.cle_hote === "premier-contact";
  barre.classList.toggle("premier-contact", premier);
  const cle = morceau(
    "bs-cle",
    premier ? t("securite-premier-contact") : t("securite-cle-connue"),
    premier ? t("securite-premier-contact-aide") : t("securite-cle-connue-aide"),
  );
  cle.prepend(bouclier());
  enfants.push(cle);
  if (e.empreinte_cle_hote) {
    enfants.push(
      morceau("bs-mono bs-empreinte", `${e.algorithme_cle_hote} ${e.empreinte_cle_hote}`.trim(), t("securite-empreinte")),
    );
  }
  if (e.echange_de_cles) enfants.push(morceau("bs-mono", e.echange_de_cles, t("securite-echange")));
  if (e.chiffrement) enfants.push(morceau("bs-mono", e.chiffrement, t("securite-chiffrement")));
  if (e.integrite) enfants.push(morceau("bs-mono", e.integrite, t("securite-integrite")));
  if (e.authentification) enfants.push(morceau("bs-auth", t(`securite-auth-${e.authentification}`)));
  barre.replaceChildren(...enfants);
}

/** Met la barre à jour pour l'onglet actif. Appelée à chaque changement
 *  d'onglet ou d'état de session (par `setTitlebar`, qui les voit tous). */
export function majBarreSecurite(): void {
  const barre = document.getElementById("barre-securite");
  if (!barre) return;
  const s = state.active === null ? null : (state.sessions.get(state.active) ?? null);
  rendreBarreSecurite(barre, s);
}
