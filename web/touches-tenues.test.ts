import { describe, expect, it } from "vitest";
import { ToucheTenues } from "./touches-tenues";
import { le16, rdpScancode } from "./filters";
import { messageKeysym } from "./vnc-clavier";

// Construit le message [4] de relâchement d'un scancode RDP, comme le fait rdp.ts.
const relacheRdp = (jeton: string): number[] | null => {
  const sc = rdpScancode(jeton);
  return sc ? [4, ...le16(sc), 0] : null;
};

describe("ToucheTenues : relâcher les touches restées enfoncées", () => {
  it("relacherTout rend un message [4] down=0 par touche tenue, puis se vide", () => {
    // Cas de l'audit du 7 septembre 2026 : Alt+Tab enfonce des touches côté
    // distant, les keyup partent à l'autre fenêtre. Au blur du canvas, il faut
    // relâcher tout ce qui restait tenu.
    const t = new ToucheTenues(relacheRdp);
    t.enfoncer("KeyA");
    t.enfoncer("ShiftLeft");
    expect(t.relacherTout()).toEqual([
      [4, ...le16(0x1e), 0], // KeyA
      [4, ...le16(0x2a), 0], // ShiftLeft
    ]);
    // Le suivi est vidé : un second appel ne renvoie plus rien.
    expect(t.relacherTout()).toEqual([]);
  });

  it("un_keyup_sans_keydown_n_est_pas_transmis", () => {
    // En VNC, un release fantôme (keysym jamais pressé dans cette session, tel
    // le keyup de Ctrl après un Ctrl+Tab qui atterrit sur le nouveau canvas)
    // doit être filtré : relacher renvoie faux, l'appelant n'envoie rien.
    const t = new ToucheTenues((jeton) => messageKeysym(Number(jeton), false));
    expect(t.relacher("65507")).toBe(false); // 0xffe3 Control_L jamais enfoncé
    t.enfoncer("65507");
    expect(t.relacher("65507")).toBe(true); // enfoncé puis relâché : transmis
    expect(t.relacher("65507")).toBe(false); // déjà relâché : plus rien
  });

  it("une touche non prise en charge n'ajoute pas de message parasite", () => {
    // rdpScancode renvoie null pour un code inconnu : relacherTout ne doit pas
    // pousser de message vide pour lui.
    const t = new ToucheTenues(relacheRdp);
    t.enfoncer("KeyA");
    t.enfoncer("MediaPlayPause"); // aucun scancode
    expect(t.relacherTout()).toEqual([[4, ...le16(0x1e), 0]]);
  });
});
