// Clavier VNC : un événement clavier du navigateur devient un keysym X11
// (RFC 6143, 7.5.4). RDP transporte la touche physique (scancode) et laisse le
// serveur la traduire selon la disposition ; RFB transporte le caractère que
// l'utilisateur a obtenu : « a » sur un AZERTY comme sur un QWERTY. C'est
// `KeyboardEvent.key` qui le porte ; `code` ne sert qu'à distinguer les
// touches qui ont le même nom à gauche et à droite, et le pavé numérique.

/** Touches sans caractère, par leur nom `key`. */
const SPECIALES: Record<string, number> = {
  Backspace: 0xff08, Tab: 0xff09, Enter: 0xff0d, Escape: 0xff1b, Delete: 0xffff,
  Home: 0xff50, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54,
  PageUp: 0xff55, PageDown: 0xff56, End: 0xff57, Insert: 0xff63,
  Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9, Meta: 0xffeb, OS: 0xffeb,
  AltGraph: 0xfe03, // ISO_Level3_Shift : la touche AltGr des claviers européens
  CapsLock: 0xffe5, NumLock: 0xff7f, ScrollLock: 0xff14,
  Pause: 0xff13, PrintScreen: 0xff61, ContextMenu: 0xff67,
};

/** Variantes de droite, quand `code` les distingue. */
const DROITES: Record<string, number> = {
  ShiftRight: 0xffe2, ControlRight: 0xffe4, AltRight: 0xffea, MetaRight: 0xffec,
};

/** Pavé numérique : les keysyms KP_*, pour que le serveur voie le pavé. */
const PAVE: Record<string, number> = {
  NumpadAdd: 0xffab, NumpadSubtract: 0xffad, NumpadMultiply: 0xffaa, NumpadDivide: 0xffaf,
  NumpadDecimal: 0xffae, NumpadEnter: 0xff8d,
};

/** Le keysym d'un événement clavier, ou `null` s'il n'en a pas (touche morte,
 *  touche inconnue). Les fonctions F1 à F35 se calculent, le reste se lit dans
 *  les tables ; un caractère imprimable est son point de code (Latin-1 tel
 *  quel, au-delà avec le préfixe Unicode 0x01000000 des keysyms X11). */
export function keysymDe(e: { key: string; code: string }): number | null {
  const k = e.key;
  if (k.length === 1 || (k.length === 2 && k.codePointAt(0)! > 0xffff)) {
    const cp = k.codePointAt(0)!;
    if (e.code.startsWith("Numpad")) {
      if (cp >= 0x30 && cp <= 0x39) return 0xffb0 + (cp - 0x30);
      const p = PAVE[e.code];
      if (p !== undefined) return p;
    }
    return cp < 0x100 ? cp : 0x01000000 + cp;
  }
  const f = /^F(\d{1,2})$/.exec(k);
  if (f) {
    const n = Number(f[1]);
    return n >= 1 && n <= 35 ? 0xffbe + (n - 1) : null;
  }
  if (k === "Enter" && e.code === "NumpadEnter") return PAVE.NumpadEnter;
  const droite = DROITES[e.code];
  if (droite !== undefined && (k === "Shift" || k === "Control" || k === "Alt" || k === "Meta")) return droite;
  return SPECIALES[k] ?? null;
}

/** Le message [14] KEYSYM du canal local : keysym sur quatre octets, puis 1 (appui) ou 0. */
export function messageKeysym(keysym: number, enfonce: boolean): number[] {
  return [14, keysym & 0xff, (keysym >>> 8) & 0xff, (keysym >>> 16) & 0xff, (keysym >>> 24) & 0xff, enfonce ? 1 : 0];
}
