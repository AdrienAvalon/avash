// Décodage des messages du processus de bureau distant (sidecar → front).
//
// Audit du 12 septembre 2026 (couverture, section 5, point 1) : `rdp.ts` n'était
// importé par aucun test, et son `ws.onmessage` décodait en ligne, sur un
// `DataView`, ce qu'envoie un processus qui relaie un serveur potentiellement
// hostile. Le décodage est sorti en fonction pure (`decoderTrame`) et testé ici
// sur des `ArrayBuffer` construits à la main, octet par octet. FS-8 (même audit) :
// les messages JSON 15, 17 et 18 étaient pris tels quels, sans garde de forme.
import { describe, it, expect } from "vitest";
import { decoderTrame } from "./trames-bureau";

/** Un message binaire : code puis octets. */
const trame = (...octets: number[]): ArrayBuffer => new Uint8Array(octets).buffer;
/** Entier 16 bits petit-boutiste. */
const u16 = (n: number): number[] => [n & 0xff, (n >> 8) & 0xff];
/** Code suivi d'un JSON UTF-8. */
const json = (code: number, valeur: unknown): ArrayBuffer => {
  const corps = new TextEncoder().encode(JSON.stringify(valeur));
  const m = new Uint8Array(1 + corps.length);
  m[0] = code;
  m.set(corps, 1);
  return m.buffer;
};
const texte = (code: number, s: string): ArrayBuffer => {
  const corps = new TextEncoder().encode(s);
  const m = new Uint8Array(1 + corps.length);
  m[0] = code;
  m.set(corps, 1);
  return m.buffer;
};

describe("decoderTrame : géométrie et images", () => {
  it("une_trame_connecte_porte_la_taille_du_bureau", () => {
    expect(decoderTrame(trame(1, ...u16(1920), ...u16(1080)))).toEqual({ type: "connecte", largeur: 1920, hauteur: 1080 });
  });

  it("une_trame_connecte_tronquee_est_invalide", () => {
    expect(decoderTrame(trame(1, 0x80)).type).toBe("invalide");
  });

  it("une_image_simple_designe_ses_pixels_sans_copie", () => {
    const px = [1, 2, 3, 255, 4, 5, 6, 255];
    const t = decoderTrame(trame(2, ...u16(10), ...u16(5), ...u16(2), ...u16(1), ...px));
    expect(t).toEqual({ type: "image", complete: true, rects: [{ x: 10, y: 5, largeur: 2, hauteur: 1, decalage: 9 }] });
  });

  it("une_image_qui_deborde_n_est_pas_peinte_mais_reste_une_image_a_accuser", () => {
    // 100×100 annoncés, quatre octets fournis : ImageData lèverait ; le
    // cadencement exige pourtant un accusé, sinon le flux se fige.
    const t = decoderTrame(trame(2, ...u16(0), ...u16(0), ...u16(100), ...u16(100), 1, 2, 3, 4));
    expect(t).toEqual({ type: "image", complete: false, rects: [] });
  });

  it("une_image_hors_ecran_de_taille_nulle_ne_peint_rien", () => {
    // Le processus émet un [2] vide pour une mise à jour hors de l'image (trames.rs).
    expect(decoderTrame(trame(2, ...u16(0), ...u16(0), ...u16(0), ...u16(1)))).toEqual({ type: "image", complete: true, rects: [] });
  });

  it("un_en_tete_d_image_tronque_reste_une_image_a_accuser", () => {
    expect(decoderTrame(trame(2, 1, 2, 3))).toEqual({ type: "image", complete: false, rects: [] });
  });

  it("une_trame_multiple_enchaine_ses_rectangles", () => {
    const r1 = [...u16(1), ...u16(2), ...u16(1), ...u16(1), 9, 9, 9, 255];
    const r2 = [...u16(30), ...u16(40), ...u16(1), ...u16(2), 7, 7, 7, 255, 8, 8, 8, 255];
    const t = decoderTrame(trame(13, 2, ...r1, ...r2));
    expect(t).toEqual({
      type: "image",
      complete: true,
      rects: [
        { x: 1, y: 2, largeur: 1, hauteur: 1, decalage: 10 },
        { x: 30, y: 40, largeur: 1, hauteur: 2, decalage: 22 },
      ],
    });
  });

  it("une_trame_multiple_tronquee_garde_les_rectangles_entiers", () => {
    const r1 = [...u16(1), ...u16(2), ...u16(1), ...u16(1), 9, 9, 9, 255];
    const r2 = [...u16(30), ...u16(40), ...u16(50), ...u16(50), 7];
    const t = decoderTrame(trame(13, 2, ...r1, ...r2));
    expect(t).toEqual({ type: "image", complete: false, rects: [{ x: 1, y: 2, largeur: 1, hauteur: 1, decalage: 10 }] });
  });
});

describe("decoderTrame : messages courts", () => {
  it("la_qualite_porte_images_par_seconde_debit_et_latence", () => {
    expect(decoderTrame(trame(7, ...u16(30), 0x00, 0x04, 0x00, 0x00, ...u16(42)))).toEqual({ type: "qualite", fps: 30, kbps: 1024, latence: 42 });
    expect(decoderTrame(trame(7, 1, 2)).type).toBe("invalide");
  });

  it("le_presse_papiers_distant_est_du_texte_utf8", () => {
    expect(decoderTrame(texte(8, "héhé ✓"))).toEqual({ type: "presse-papiers", texte: "héhé ✓" });
  });

  it("une_erreur_porte_son_texte", () => {
    expect(decoderTrame(texte(3, "refusé"))).toEqual({ type: "erreur", texte: "refusé" });
  });

  it("le_changement_de_presse_papiers_distant_n_a_pas_de_charge", () => {
    expect(decoderTrame(trame(14))).toEqual({ type: "presse-papiers-change" });
  });

  it("un_bloc_de_son_part_tel_quel_au_lecteur", () => {
    expect(decoderTrame(trame(20, 1, 2, 3, 4))).toEqual({ type: "son" });
  });

  it("le_volume_porte_ses_deux_canaux", () => {
    expect(decoderTrame(trame(21, ...u16(0xffff), ...u16(0x8000)))).toEqual({ type: "volume", gauche: 0xffff, droite: 0x8000 });
    expect(decoderTrame(trame(21, 1)).type).toBe("invalide");
  });

  it("une_reprise_de_connexion_est_annoncee_par_le_message_23", () => {
    // Contrat K7 : le processus refait toute la connexion (redirection,
    // reprise du canal graphique) après un premier [1] ; l'onglet doit le dire.
    expect(decoderTrame(trame(23))).toEqual({ type: "reprise" });
  });

  it("un_code_inconnu_ou_une_trame_vide_ne_font_rien_lever", () => {
    expect(decoderTrame(trame(99, 1, 2))).toEqual({ type: "inconnue", code: 99 });
    expect(decoderTrame(new ArrayBuffer(0)).type).toBe("invalide");
  });
});

describe("decoderTrame : fichiers par le presse-papiers (JSON gardé)", () => {
  const liste = { dossier: "/home/moi/Téléchargements", octets: 12, fichiers: [{ chemin: "a.txt", taille: 12, dossier: false }] };

  it("la_liste_des_fichiers_copies_est_rendue_telle_quelle", () => {
    expect(decoderTrame(json(15, liste))).toEqual({ type: "fichiers-copies", liste });
  });

  it("une_liste_de_forme_inattendue_est_ecartee", () => {
    expect(decoderTrame(json(15, null)).type).toBe("invalide");
    expect(decoderTrame(json(15, { ...liste, fichiers: "a.txt" })).type).toBe("invalide");
    expect(decoderTrame(json(15, { ...liste, fichiers: [{ chemin: 3 }] })).type).toBe("invalide");
    expect(decoderTrame(texte(15, "{pas du json")).type).toBe("invalide");
  });

  it("la_progression_tronque_un_nom_de_fichier_demesure", () => {
    const p = { fichier: "x".repeat(5000), fait: 1, total: 2, termines: 0, nombre: 1 };
    const t = decoderTrame(json(17, p));
    expect(t.type).toBe("progression-fichiers");
    if (t.type !== "progression-fichiers") return;
    expect(t.progression.fichier.length).toBeLessThanOrEqual(200);
    expect(t.progression.total).toBe(2);
    expect(decoderTrame(json(17, { ...p, fait: "beaucoup" })).type).toBe("invalide");
  });

  it("le_bilan_exige_sa_liste_d_erreurs", () => {
    const b = { sens: "reception", dossier: "/tmp", fichiers: 1, octets: 12, erreurs: [] };
    expect(decoderTrame(json(18, b))).toEqual({ type: "bilan-fichiers", bilan: b });
    expect(decoderTrame(json(18, { sens: "offre", fichiers: 0, octets: 0, erreurs: ["refus"] }))).toEqual({
      type: "bilan-fichiers", bilan: { sens: "offre", fichiers: 0, octets: 0, erreurs: ["refus"] },
    });
    expect(decoderTrame(json(18, { ...b, erreurs: "aucune" })).type).toBe("invalide");
    expect(decoderTrame(json(18, 42)).type).toBe("invalide");
  });
});
