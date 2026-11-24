// Réessais du presse-papiers du poste quand une autre application le tient
// ouvert (Windows), et ordre des frappes autour d'un collage. Signalé le
// 24 novembre 2026 : « le copier-coller ne marche pas toujours » depuis
// Windows 11 vers un Windows Server 2012 R2.
import { describe, it, expect, vi } from "vitest";
import { avecReessais, estCollage, FileCollage, presseOccupe, ESSAIS_PRESSE_PAPIERS } from "./collage-rdp";

const OCCUPE = "The native clipboard is not accessible due to being held by another party.";
const VIDE = "The clipboard contents were not available in the requested format or the clipboard is empty.";
const sansPause = () => Promise.resolve();

describe("presse-papiers occupé", () => {
  it("reconnait_l_erreur_d_arboard_et_rien_d_autre", () => {
    expect(presseOccupe(OCCUPE)).toBe(true);
    expect(presseOccupe(new Error(OCCUPE))).toBe(true);
    expect(presseOccupe(VIDE)).toBe(false);
  });

  it("reessaie_tant_que_le_presse_papiers_est_occupe", async () => {
    const op = vi.fn().mockRejectedValueOnce(OCCUPE).mockRejectedValueOnce(OCCUPE).mockResolvedValue("texte");
    await expect(avecReessais(op, ESSAIS_PRESSE_PAPIERS, sansPause)).resolves.toBe("texte");
    expect(op).toHaveBeenCalledTimes(3);
  });

  it("abandonne_apres_le_dernier_essai", async () => {
    const op = vi.fn().mockRejectedValue(OCCUPE);
    await expect(avecReessais(op, 4, sansPause)).rejects.toBe(OCCUPE);
    expect(op).toHaveBeenCalledTimes(4);
  });

  it("une_autre_erreur_sort_au_premier_essai", async () => {
    // Un presse-papiers vide ou qui tient une image : réessayer ne ferait
    // que retarder la touche qui colle.
    const op = vi.fn().mockRejectedValue(VIDE);
    await expect(avecReessais(op, 4, sansPause)).rejects.toBe(VIDE);
    expect(op).toHaveBeenCalledTimes(1);
  });
});

describe("gestes de collage", () => {
  const e = (code: string, mods: Partial<{ ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) =>
    ({ ctrlKey: false, shiftKey: false, altKey: false, code, ...mods });
  it("ctrl_v_et_maj_inser_collent_altgr_v_non", () => {
    expect(estCollage(e("KeyV", { ctrlKey: true }))).toBe(true);
    expect(estCollage(e("Insert", { shiftKey: true }))).toBe(true);
    // AltGr sous Windows = Ctrl + Alt : un caractère, pas un collage.
    expect(estCollage(e("KeyV", { ctrlKey: true, altKey: true }))).toBe(false);
    expect(estCollage(e("KeyV"))).toBe(false);
  });
});

describe("file des frappes autour d'un collage", () => {
  it("sans_collage_tout_part_tout_de_suite", () => {
    const parti: string[] = [];
    const file = new FileCollage(() => Promise.resolve());
    file.executer(() => parti.push("a"));
    expect(parti).toEqual(["a"]);
  });

  it("les_frappes_attendent_l_annonce_dans_l_ordre", async () => {
    let finir: () => void = () => {};
    const parti: string[] = [];
    const file = new FileCollage(() => new Promise<void>((r) => { finir = () => { parti.push("annonce"); r(); }; }));
    file.coller(() => parti.push("V enfoncée"));
    file.executer(() => parti.push("V relâchée"));
    await Promise.resolve();
    expect(parti).toEqual([]);
    finir();
    await file.vide();
    expect(parti).toEqual(["annonce", "V enfoncée", "V relâchée"]);
  });

  it("une_annonce_qui_echoue_n_avale_pas_la_touche", async () => {
    const parti: string[] = [];
    const file = new FileCollage(() => Promise.reject(new Error("lecture impossible")));
    file.coller(() => parti.push("V"));
    await file.vide();
    expect(parti).toEqual(["V"]);
  });
});
