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

/** Vrai sur un poste Windows. On préfère `userAgentData.platform` (exact sous
 *  Chromium/WebView2) et on retombe sur `navigator.platform` (« Win32 »). */
export function estWindows(): boolean {
  const uad = (navigator as unknown as { userAgentData?: { platform?: string } }).userAgentData;
  if (uad?.platform) return uad.platform === "Windows";
  return /win/i.test(navigator.platform);
}

/** Événement clavier réduit à ce dont le filtre a besoin. */
interface EvTouche { key: string; code: string; }

/** Sous Windows, la touche AltGr est émulée par un appui synthétique de Ctrl
 *  gauche suivi d'Alt droite ; Chromium/WebView2 les expose comme deux keydown
 *  distincts (key=« Control » code=« ControlLeft », puis key=« AltGraph »
 *  code=« AltRight »). Transmis tels quels à un serveur VNC X11, le Control_L
 *  reste enfoncé et « @ » (AltGr+0 sur AZERTY) arrive comme Ctrl+@ (NUL) ; idem
 *  pour « # { } | \ ~ € ». Comme noVNC (keyboard.js), on retient le keydown Ctrl
 *  gauche quelques millisecondes sans l'émettre : suivi immédiatement d'un
 *  keydown AltGraph, on l'abandonne (et on ignore son keyup jumeau, repéré par
 *  code=« ControlLeft ») ; sinon on l'émet. Trouvé par l'audit du 7 septembre
 *  2026 : rien ne supprimait ce Control synthétique, à la différence du chemin
 *  RDP (scancodes, où le serveur Windows connaît la convention). Le filtre ne
 *  s'active que pour un client Windows visant une cible VNC ; Linux (vraie
 *  ISO_Level3_Shift) et RDP ne sont pas concernés. */
export class FiltreCtrlAltGrWindows {
  /** Le keydown Ctrl gauche retenu, tant qu'on ignore si un AltGraph suit. */
  private ctrlDiffere: EvTouche | null = null;
  /** Poignée du minuteur qui finira par émettre le Ctrl retenu, ou `null`. */
  private minuteur: ReturnType<typeof setTimeout> | null = null;
  /** Le prochain keyup ControlLeft est le jumeau d'un Ctrl abandonné : l'avaler. */
  private avaler = false;

  /**
   * @param actif filtre en service (client Windows + cible VNC) ; sinon on émet
   *   chaque événement tel quel.
   * @param emettre traitement réel d'une touche (keysym, suivi des tenues, envoi).
   * @param planifier pose le minuteur de vidage (injectable pour les tests).
   * @param annuler retire le minuteur.
   */
  constructor(
    private readonly actif: boolean,
    private readonly emettre: (e: EvTouche, enfonce: boolean) => void,
    private readonly planifier: (cb: () => void) => ReturnType<typeof setTimeout> = (cb) => setTimeout(cb, 5),
    private readonly annuler: (h: ReturnType<typeof setTimeout>) => void = (h) => { clearTimeout(h); },
  ) {}

  /** Émet le Ctrl retenu (sans toucher au minuteur), s'il y en a un. */
  private viderDiffere(): void {
    if (!this.ctrlDiffere) return;
    const e = this.ctrlDiffere;
    this.ctrlDiffere = null;
    this.emettre(e, true);
  }

  /** Annule le minuteur et émet le Ctrl retenu : appelé avant tout autre envoi. */
  private vider(): void {
    if (this.minuteur !== null) { this.annuler(this.minuteur); this.minuteur = null; }
    this.viderDiffere();
  }

  /** Filtre un keydown/keyup et délègue à `emettre` ce qui doit réellement partir. */
  traiter(e: EvTouche, enfonce: boolean): void {
    if (!this.actif) { this.emettre(e, enfonce); return; }
    // Keydown Ctrl gauche : on le retient au lieu de l'émettre tout de suite.
    if (enfonce && e.key === "Control" && e.code === "ControlLeft") {
      this.vider(); // un Ctrl déjà retenu (rare) part d'abord
      this.ctrlDiffere = { key: e.key, code: e.code };
      this.minuteur = this.planifier(() => { this.minuteur = null; this.viderDiffere(); });
      return;
    }
    // Keydown AltGraph juste après : c'est l'AltGr synthétique de Windows. On
    // abandonne le Ctrl retenu et on avalera son keyup jumeau.
    if (enfonce && e.key === "AltGraph") {
      if (this.ctrlDiffere) {
        if (this.minuteur !== null) { this.annuler(this.minuteur); this.minuteur = null; }
        this.ctrlDiffere = null;
        this.avaler = true;
      }
      this.emettre(e, true);
      return;
    }
    // Keyup ControlLeft jumeau du Ctrl abandonné : à ignorer une fois.
    if (!enfonce && e.code === "ControlLeft" && this.avaler) {
      this.avaler = false;
      return;
    }
    // Tout le reste : on émet d'abord un Ctrl encore retenu (frappe réelle comme
    // Ctrl+C, ou relâchement d'un Ctrl tapé seul), puis l'événement courant.
    this.vider();
    this.emettre(e, enfonce);
  }
}
