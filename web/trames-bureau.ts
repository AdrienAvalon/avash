// Décodage des messages du processus de bureau distant (sidecar → front).
//
// Audit du 12 septembre 2026 (couverture, section 5, point 1) : `ws.onmessage`
// décodait en ligne, sur un `DataView`, ce qu'envoie un processus qui relaie un
// serveur potentiellement hostile, et aucun test n'importait `rdp.ts`. Le
// décodage vit ici, sans DOM ni état, testé sur des tampons construits à la main
// (trames-bureau.test.ts) ; `rdp.ts` ne fait plus qu'appliquer le résultat.
//
// Format : un octet de code, puis la charge. Les entiers sont petit-boutistes.
//   [1] connecté : largeur u16, hauteur u16 (aussi après un redimensionnement)
//   [2] image : x, y, largeur, hauteur (u16) puis largeur × hauteur × 4 octets RGBA
//   [3] erreur : texte UTF-8
//   [7] qualité : images/s u16, débit Ko/s u32, latence ms u16
//   [8] presse-papiers distant : texte UTF-8
//   [13] image à plusieurs rectangles : nombre u8, puis [2] sans code, à la suite
//   [14] le presse-papiers distant a changé (sans charge)
//   [15] [17] [18] fichiers copiés, progression d'une réception, bilan : JSON
//   [20] bloc de son PCM (le tampon entier va au lecteur), [21] volume : u16, u16
//   [23] reprise de la connexion (contrat K7 du même audit, sans charge)

type Rect = { x: number; y: number; largeur: number; hauteur: number; decalage: number };

/** Ce que le bureau distant a copié en dernier (message [15]). */
export type FichiersDistants = { dossier: string; octets: number; fichiers: { chemin: string; taille: number; dossier: boolean }[] };
type ProgressionFichiers = { fichier: string; fait: number; total: number; termines: number; nombre: number };
/** Bilan d'une réception ou d'une offre de fichiers (message [18]). */
export type BilanFichiers = { sens: string; dossier?: string; fichiers: number; octets: number; erreurs: string[] };

type Trame =
  | { type: "connecte"; largeur: number; hauteur: number }
  // `complete` faux : une partie annoncée manquait. Une image s'accuse
  // toujours, même vide ou abîmée, sinon le cadencement du sidecar se fige.
  | { type: "image"; rects: Rect[]; complete: boolean }
  | { type: "erreur"; texte: string }
  | { type: "qualite"; fps: number; kbps: number; latence: number }
  | { type: "presse-papiers"; texte: string }
  | { type: "presse-papiers-change" }
  | { type: "fichiers-copies"; liste: FichiersDistants }
  | { type: "progression-fichiers"; progression: ProgressionFichiers }
  | { type: "bilan-fichiers"; bilan: BilanFichiers }
  | { type: "son" }
  | { type: "volume"; gauche: number; droite: number }
  | { type: "reprise" }
  | { type: "invalide"; code: number; raison: string }
  | { type: "inconnue"; code: number };

/** Un nom de fichier distant s'affiche dans une pastille : borné (FS-8). */
const NOM_MAX = 200;

const invalide = (code: number, raison: string): Trame => ({ type: "invalide", code, raison });
const utf8 = (buf: ArrayBuffer): string => new TextDecoder().decode(new Uint8Array(buf, 1));
const nombre = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const objet = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

function lireJson(buf: ArrayBuffer): unknown {
  try {
    return JSON.parse(utf8(buf));
  } catch {
    return undefined;
  }
}

function estListe(v: unknown): v is FichiersDistants {
  return objet(v) && typeof v.dossier === "string" && nombre(v.octets) && Array.isArray(v.fichiers)
    && v.fichiers.every((f) => objet(f) && typeof f.chemin === "string" && nombre(f.taille) && typeof f.dossier === "boolean");
}

function estBilan(v: unknown): v is BilanFichiers {
  return objet(v) && typeof v.sens === "string" && nombre(v.fichiers) && nombre(v.octets)
    && (v.dossier === undefined || typeof v.dossier === "string")
    && Array.isArray(v.erreurs) && v.erreurs.every((e) => typeof e === "string");
}

function progression(v: unknown): ProgressionFichiers | null {
  if (!objet(v) || typeof v.fichier !== "string") return null;
  const { fait, total, termines, nombre: n } = v;
  if (!nombre(fait) || !nombre(total) || !nombre(termines) || !nombre(n)) return null;
  const fichier = v.fichier.length > NOM_MAX ? `${v.fichier.slice(0, NOM_MAX - 1)}…` : v.fichier;
  return { fichier, fait, total, termines, nombre: n };
}

/** Lit un rectangle à `p` (en-tête de 8 octets puis pixels). `null` : il déborde. */
function lireRect(dv: DataView, p: number): { rect: Rect; suite: number } | null {
  if (p + 8 > dv.byteLength) return null;
  const largeur = dv.getUint16(p + 4, true), hauteur = dv.getUint16(p + 6, true);
  const octets = largeur * hauteur * 4;
  if (p + 8 + octets > dv.byteLength) return null;
  return { rect: { x: dv.getUint16(p, true), y: dv.getUint16(p + 2, true), largeur, hauteur, decalage: p + 8 }, suite: p + 8 + octets };
}

/** Décode un message du processus de bureau distant. Ne lève jamais. */
export function decoderTrame(buf: ArrayBuffer): Trame {
  if (buf.byteLength === 0) return invalide(-1, "trame vide");
  const dv = new DataView(buf);
  const code = dv.getUint8(0);
  switch (code) {
    case 1:
      return buf.byteLength >= 5 ? { type: "connecte", largeur: dv.getUint16(1, true), hauteur: dv.getUint16(3, true) } : invalide(code, "taille tronquée");
    case 2:
    case 13: {
      const rects: Rect[] = [];
      let p = code === 2 ? 1 : 2;
      const n = code === 2 ? 1 : buf.byteLength >= 2 ? dv.getUint8(1) : -1;
      if (n < 0) return { type: "image", rects, complete: false };
      for (let i = 0; i < n; i++) {
        const lu = lireRect(dv, p);
        if (!lu) return { type: "image", rects, complete: false };
        // Largeur ou hauteur nulle : mise à jour hors de l'image, rien à peindre
        // (`ImageData` refuserait une dimension nulle).
        if (lu.rect.largeur > 0 && lu.rect.hauteur > 0) rects.push(lu.rect);
        p = lu.suite;
      }
      return { type: "image", rects, complete: true };
    }
    case 3:
      return { type: "erreur", texte: utf8(buf) };
    case 7:
      return buf.byteLength >= 9
        ? { type: "qualite", fps: dv.getUint16(1, true), kbps: dv.getUint32(3, true), latence: dv.getUint16(7, true) }
        : invalide(code, "mesure tronquée");
    case 8:
      return { type: "presse-papiers", texte: utf8(buf) };
    case 14:
      return { type: "presse-papiers-change" };
    case 15: {
      const v = lireJson(buf);
      return estListe(v) ? { type: "fichiers-copies", liste: v } : invalide(code, "liste de fichiers de forme inattendue");
    }
    case 17: {
      const p = progression(lireJson(buf));
      return p ? { type: "progression-fichiers", progression: p } : invalide(code, "progression de forme inattendue");
    }
    case 18: {
      const v = lireJson(buf);
      return estBilan(v) ? { type: "bilan-fichiers", bilan: v } : invalide(code, "bilan de forme inattendue");
    }
    case 20:
      return { type: "son" };
    case 21:
      return buf.byteLength >= 5 ? { type: "volume", gauche: dv.getUint16(1, true), droite: dv.getUint16(3, true) } : invalide(code, "volume tronqué");
    case 23:
      return { type: "reprise" };
    default:
      return { type: "inconnue", code };
  }
}
