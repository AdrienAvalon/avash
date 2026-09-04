import { describe, expect, it } from "vitest";
import { keysymDe, messageKeysym } from "./vnc-clavier";

const ev = (key: string, code = "") => ({ key, code });

describe("keysym VNC d'un événement clavier", () => {
  it("un caractère imprimable est son point de code, quel que soit le clavier", () => {
    // « a » sur un AZERTY vient du code KeyQ : le keysym reste « a ».
    expect(keysymDe(ev("a", "KeyQ"))).toBe(0x61);
    expect(keysymDe(ev("A", "KeyQ"))).toBe(0x41);
    expect(keysymDe(ev(" ", "Space"))).toBe(0x20);
    // Latin-1 tel quel : « é » vaut 0xe9.
    expect(keysymDe(ev("é", "Digit2"))).toBe(0xe9);
    // Au-delà, le préfixe Unicode des keysyms X11.
    expect(keysymDe(ev("€", "KeyE"))).toBe(0x01000000 + 0x20ac);
    expect(keysymDe(ev("😀", "KeyE"))).toBe(0x01000000 + 0x1f600);
  });

  it("les touches sans caractère ont leur keysym X11", () => {
    expect(keysymDe(ev("Enter", "Enter"))).toBe(0xff0d);
    expect(keysymDe(ev("Backspace", "Backspace"))).toBe(0xff08);
    expect(keysymDe(ev("Escape", "Escape"))).toBe(0xff1b);
    expect(keysymDe(ev("ArrowLeft", "ArrowLeft"))).toBe(0xff51);
    expect(keysymDe(ev("Delete", "Delete"))).toBe(0xffff);
    expect(keysymDe(ev("F1", "F1"))).toBe(0xffbe);
    expect(keysymDe(ev("F12", "F12"))).toBe(0xffc9);
    expect(keysymDe(ev("F36", "F36"))).toBeNull();
  });

  it("les modificateurs distinguent la gauche de la droite, et AltGr existe", () => {
    expect(keysymDe(ev("Shift", "ShiftLeft"))).toBe(0xffe1);
    expect(keysymDe(ev("Shift", "ShiftRight"))).toBe(0xffe2);
    expect(keysymDe(ev("Control", "ControlRight"))).toBe(0xffe4);
    expect(keysymDe(ev("Alt", "AltLeft"))).toBe(0xffe9);
    expect(keysymDe(ev("AltGraph", "AltRight"))).toBe(0xfe03);
    expect(keysymDe(ev("Meta", "MetaLeft"))).toBe(0xffeb);
  });

  it("le pavé numérique envoie ses propres keysyms", () => {
    expect(keysymDe(ev("7", "Numpad7"))).toBe(0xffb7);
    expect(keysymDe(ev("+", "NumpadAdd"))).toBe(0xffab);
    expect(keysymDe(ev("Enter", "NumpadEnter"))).toBe(0xff8d);
    // Pavé sans Verr.Num : les flèches, comme les autres.
    expect(keysymDe(ev("ArrowUp", "Numpad8"))).toBe(0xff52);
  });

  it("une touche morte ou inconnue ne produit rien", () => {
    expect(keysymDe(ev("Dead", "Quote"))).toBeNull();
    expect(keysymDe(ev("Unidentified", ""))).toBeNull();
  });

  it("le message [14] porte le keysym en petit-boutiste sur quatre octets", () => {
    expect(messageKeysym(0xff0d, true)).toEqual([14, 0x0d, 0xff, 0, 0, 1]);
    expect(messageKeysym(0x01000000 + 0x1f600, false)).toEqual([14, 0x00, 0xf6, 0x01, 0x01, 0]);
  });
});
