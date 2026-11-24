// Collage du poste vers un bureau RDP au clavier (Ctrl+V, Maj+Inser).
//
// Signalé sur Windows Server 2012 R2 le 24 novembre 2026 : « le copier-coller
// ne marche pas toujours ». En RDP, le presse-papiers du poste n'était annoncé
// au serveur que sur un clic dans le bureau (`pousseAuGeste`). On copiait dans
// une autre application, on revenait dans Avash par Alt+Tab ou par la barre des
// tâches (le bureau garde le focus, aucun clic), on tapait Ctrl+V : le serveur
// collait l'ancien contenu, ou rien. Le Ctrl+V tapé dans le bureau est un geste
// aussi explicite que le clic : il annonce désormais le texte du poste, et la
// touche ne part qu'après l'annonce.

/** Le temps laissé au serveur pour prendre l'annonce avant la touche qui
 *  colle. Le serveur reçoit les deux dans l'ordre, mais l'application qui
 *  colle peut lire le presse-papiers avant que le service du serveur n'ait
 *  traité l'annonce : c'est ce que fait un client qui annonce au moment de la
 *  copie, bien avant le collage. */
export const ATTENTE_APRES_ANNONCE_MS = 150;

/** Au-delà, la lecture du presse-papiers du poste est abandonnée : la touche
 *  part quand même, plutôt que de rester en suspens. */
export const LECTURE_MAX_MS = 500;

/** Ctrl+V ou Maj+Inser : les deux gestes de collage d'un bureau Windows. */
export function estCollage(e: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean; code: string }): boolean {
  return (e.ctrlKey && !e.altKey && e.code === "KeyV") || (e.shiftKey && !e.ctrlKey && e.code === "Insert");
}

/** Ordonne les frappes d'un bureau autour d'un collage : tant qu'une annonce
 *  est en cours, les touches, relâchements compris, attendent derrière elle
 *  dans l'ordre où elles sont venues. Sans collage en cours, tout part tout de
 *  suite, sans le moindre délai. */
export class FileCollage {
  private file: Promise<void> = Promise.resolve();
  private enAttente = 0;

  constructor(private readonly annoncer: () => Promise<void>) {}

  private chainer(etape: () => Promise<void> | void): void {
    this.enAttente++;
    this.file = this.file
      .then(etape)
      .catch(() => {})
      .finally(() => { this.enAttente--; });
  }

  /** Une frappe ordinaire : tout de suite, ou derrière le collage en cours. */
  executer(action: () => void): void {
    if (this.enAttente === 0) action();
    else this.chainer(action);
  }

  /** La touche qui colle : l'annonce d'abord, la touche ensuite, même si
   *  l'annonce échoue (on ne perd pas une frappe). */
  coller(action: () => void): void {
    this.chainer(async () => {
      try {
        await this.annoncer();
      } finally {
        action();
      }
    });
  }

  /** Attend que tout ce qui est en file soit parti (tests). */
  vide(): Promise<void> {
    return this.file;
  }
}

/** Essais d'une lecture ou d'une écriture du presse-papiers du poste quand une
 *  autre application le tient ouvert. */
export const ESSAIS_PRESSE_PAPIERS = 6;
const PAUSE_ENTRE_ESSAIS_MS = 50;

/** Le presse-papiers de Windows est-il tenu par une autre application ?
 *  C'est le texte de l'erreur `ClipboardOccupied` d'arboard, que le greffon
 *  de Tauri transmet tel quel. */
export function presseOccupe(e: unknown): boolean {
  return /held by another party/i.test(String(e));
}

/** Réessaie une opération sur le presse-papiers du poste tant qu'il est
 *  occupé. Sous Windows, une seule application à la fois peut l'ouvrir, et
 *  arboard n'insiste que 25 ms (cinq essais à 5 ms) avant d'abandonner : une
 *  application qui surveille le presse-papiers (historique de Windows,
 *  gestionnaire de mots de passe, client de synchronisation) peut suffire à faire
 *  échouer, en silence, l'écriture du texte copié sur le bureau distant. Signalé
 *  le 24 novembre 2026 (« le copier-coller ne marche pas toujours »). Toute
 *  autre erreur (presse-papiers vide, image) sort au premier essai. */
export async function avecReessais<T>(
  op: () => Promise<T>,
  essais = ESSAIS_PRESSE_PAPIERS,
  pause = (ms: number) => new Promise<void>((r) => setTimeout(r, ms)),
): Promise<T> {
  for (let i = 1; ; i++) {
    try {
      return await op();
    } catch (e) {
      if (i >= essais || !presseOccupe(e)) throw e;
      await pause(PAUSE_ENTRE_ESSAIS_MS);
    }
  }
}
