/**
 * Écriture ordonnée vers le PTY d'un onglet.
 *
 * Chaque frappe partait au cœur par son propre `invoke("pty_write")`. Or
 * Tauri traite les commandes asynchrones en parallèle, sur plusieurs fils :
 * deux frappes rapprochées pouvaient se doubler avant d'atteindre le canal
 * SSH. Sous charge, « echo » arrivait « cho » ou « ehco », et
 * « bonjour-cast-ok » devenait « bonjour-casto-k » (suite bout en bout Linux,
 * 27 novembre 2026, reproduit en local en chargeant le processeur). Un poste
 * chargé, avec une dizaine de sessions ouvertes, pouvait donc envoyer au
 * serveur une commande mélangée.
 *
 * Un seul envoi est en vol par onglet ; ce qui arrive pendant ce temps est
 * mis bout à bout et part d'un bloc à la fin de l'envoi en cours. L'ordre est
 * tenu, et une rafale (collage, frappe rapide) coûte moins d'appels.
 */
export function creerEcrivainOrdonne(
  envoyer: (data: string) => Promise<unknown>,
  surErreur: (e: unknown) => void,
): (data: string) => void {
  let enAttente = "";
  let enVol = false;

  const vider = async () => {
    enVol = true;
    while (enAttente) {
      const lot = enAttente;
      enAttente = "";
      try {
        await envoyer(lot);
      } catch (e) {
        surErreur(e);
      }
    }
    enVol = false;
  };

  return (data: string) => {
    enAttente += data;
    if (!enVol) void vider();
  };
}
