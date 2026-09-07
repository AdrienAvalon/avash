import { afterEach, describe, expect, it } from "vitest";
import { LecteurAudio, decoderBloc, gainDepuisVolume, pcm16VersFlottants } from "./audio";

function bloc(cadence: number, canaux: number, bits: number, pcm: number[]): ArrayBuffer {
  const b = new Uint8Array(12 + pcm.length);
  b[0] = 20;
  b[1] = 0;
  new DataView(b.buffer).setUint32(2, 1234, true);
  new DataView(b.buffer).setUint32(6, cadence, true);
  b[10] = canaux;
  b[11] = bits;
  b.set(pcm, 12);
  return b.buffer;
}

describe("son du bureau distant", () => {
  it("lit l'en-tête d'un bloc et refuse ce qui n'est pas du PCM 16 bits jouable", () => {
    const b = decoderBloc(bloc(44100, 2, 16, [0, 0, 0, 0]));
    expect(b).not.toBeNull();
    expect([b!.cadence, b!.canaux, b!.pcm.byteLength]).toEqual([44100, 2, 4]);
    expect(decoderBloc(bloc(44100, 2, 8, [0, 0, 0, 0]))).toBeNull();
    expect(decoderBloc(bloc(44100, 3, 16, [0, 0, 0, 0, 0, 0]))).toBeNull();
    expect(decoderBloc(bloc(1000, 1, 16, [0, 0]))).toBeNull();
    expect(decoderBloc(bloc(48000, 2, 16, [0, 0, 0]))).toBeNull();
    expect(decoderBloc(new ArrayBuffer(5))).toBeNull();
  });

  it("désentrelace le PCM en flottants entre -1 et 1", () => {
    // Deux trames stéréo : (32767, -32768) puis (0, 16384).
    const pcm = new DataView(new Int16Array([32767, -32768, 0, 16384]).buffer);
    const [g, d] = pcm16VersFlottants(pcm, 2);
    expect(Array.from(g)).toEqual([32767 / 32768, 0]);
    expect(Array.from(d)).toEqual([-1, 0.5]);
    const [mono] = pcm16VersFlottants(new DataView(new Int16Array([-16384]).buffer), 1);
    expect(Array.from(mono)).toEqual([-0.5]);
  });

  it("traduit le volume RDP en gain borné", () => {
    expect(gainDepuisVolume(65535, 65535)).toBe(1);
    expect(gainDepuisVolume(0, 0)).toBe(0);
    expect(gainDepuisVolume(65535, 0)).toBeCloseTo(0.5, 5);
  });
});

// Le lecteur enchaîne les blocs sur un curseur de temps. Ce qui se teste ici
// est cette politique de lecture (avance pour la gigue, rattrapage quand le
// curseur décroche, volume, machine sans audio), pas WebAudio : un contexte
// factice note ce que le lecteur lui demande.
class ContexteFactice {
  static instances: ContexteFactice[] = [];
  static refuser = false;
  state = "suspended";
  // Un contexte réel a déjà couru quand le premier bloc arrive : l'horloge
  // n'est jamais exactement à zéro.
  currentTime = 1;
  destination = {};
  sampleRate: number;
  reprises = 0;
  fermetures = 0;
  departs: number[] = [];
  tampons: { canaux: number; trames: number; cadence: number }[] = [];
  gain = { gain: { value: 1 }, connect: () => {} };

  constructor(options: { sampleRate: number }) {
    if (ContexteFactice.refuser) throw new Error("pas d'audio sur cette machine");
    this.sampleRate = options.sampleRate;
    ContexteFactice.instances.push(this);
  }
  createGain() { return this.gain; }
  createBuffer(canaux: number, trames: number, cadence: number) {
    this.tampons.push({ canaux, trames, cadence });
    return { copyToChannel: () => {} };
  }
  createBufferSource() {
    return { buffer: null, connect: () => {}, start: (t: number) => { this.departs.push(t); } };
  }
  resume() { this.reprises += 1; this.state = "running"; return Promise.resolve(); }
  close() { this.fermetures += 1; return Promise.resolve(); }
}
Object.defineProperty(globalThis, "AudioContext", { value: ContexteFactice, configurable: true, writable: true });

/** Un bloc stéréo 48 kHz de `trames` trames, silencieux. */
function blocStereo(trames: number): ArrayBuffer {
  return bloc(48000, 2, 16, new Array<number>(trames * 4).fill(0));
}

describe("lecteur audio du bureau distant", () => {
  afterEach(() => {
    ContexteFactice.instances = [];
    ContexteFactice.refuser = false;
  });

  it("ouvre le contexte à la cadence du premier bloc, le reprend s'il dort, et enchaîne les blocs", () => {
    const lecteur = new LecteurAudio();
    lecteur.jouer(blocStereo(480)); // 10 ms
    lecteur.jouer(blocStereo(480));
    const ctx = ContexteFactice.instances[0];
    expect(ctx.sampleRate).toBe(48000);
    expect(ctx.reprises).toBe(1);
    // Le premier bloc part avec 50 ms d'avance sur l'horloge pour absorber la
    // gigue, le second exactement à la fin du premier.
    expect(ctx.departs[0]).toBeCloseTo(1.05, 6);
    expect(ctx.departs[1]).toBeCloseTo(1.06, 6);
    expect(ctx.tampons[0]).toEqual({ canaux: 2, trames: 480, cadence: 48000 });
    expect([lecteur.blocs, lecteur.echantillons]).toEqual([2, 960]);
  });

  it("jette les blocs en trop sans les superposer quand le curseur a pris trop d'avance, et repart du présent quand il est resté en arrière", () => {
    const lecteur = new LecteurAudio();
    // Rafale après une coupure réseau (cas Wi-Fi le plus courant, audit du
    // 7 septembre 2026) : 600 ms de son, 60 blocs de 10 ms, arrivent d'un coup
    // à l'instant 1. Le curseur file jusqu'à la demi-seconde d'avance tolérée ;
    // au-delà, les blocs sont jetés (le flux rattrape en sautant) au lieu
    // d'être reprogrammés par-dessus les sources déjà en vol.
    for (let i = 0; i < 60; i++) lecteur.jouer(blocStereo(480));
    const ctx = ContexteFactice.instances[0];
    // Aucun chevauchement : chaque départ est au moins la durée d'un bloc
    // (10 ms) après le précédent. C'est ce que l'ancien code violait, en
    // ramenant le curseur au présent pendant que les blocs 1-5 jouaient encore.
    for (let i = 1; i < ctx.departs.length; i++) {
      expect(ctx.departs[i]).toBeGreaterThanOrEqual(ctx.departs[i - 1] + 0.01 - 1e-9);
    }
    // Rien n'est programmé au-delà de la demi-seconde d'avance...
    expect(Math.max(...ctx.departs)).toBeLessThanOrEqual(1.5 + 1e-6);
    // ... donc des blocs ont bien été jetés : moins de sources que de blocs, et
    // aucune source jetée n'a été programmée (départs = sources créées).
    expect(ctx.departs.length).toBeLessThan(60);
    expect(ctx.departs.length).toBeLessThanOrEqual(51);
    // Les blocs jetés restent comptés comme reçus (le décrochage reste lisible
    // au diagnostic), mais pas comme joués.
    expect(lecteur.echantillons).toBe(60 * 480);
    expect(lecteur.blocs).toBe(ctx.departs.length);
    // Un onglet resté caché : le temps a couru, le curseur est en arrière. Là
    // rien n'est en vol, on repart du présent sans risque de superposition.
    ctx.currentTime = 10;
    lecteur.jouer(blocStereo(480));
    expect(ctx.departs.at(-1)).toBeCloseTo(10.05, 6);
  });

  it("applique le volume demandé par le serveur, et ferme le contexte une seule fois", () => {
    const lecteur = new LecteurAudio();
    lecteur.volume(65535, 65535); // avant tout bloc : rien à régler, rien ne casse
    lecteur.jouer(blocStereo(48));
    lecteur.volume(65535, 0);
    const ctx = ContexteFactice.instances[0];
    expect(ctx.gain.gain.value).toBeCloseTo(0.5, 5);
    lecteur.fermer();
    lecteur.fermer();
    expect(ctx.fermetures).toBe(1);
    // Après fermeture, un bloc rouvre un contexte neuf.
    lecteur.jouer(blocStereo(48));
    expect(ContexteFactice.instances).toHaveLength(2);
  });

  it("ne bloque rien sur une machine sans audio, ni sur un bloc illisible", () => {
    ContexteFactice.refuser = true;
    const lecteur = new LecteurAudio();
    expect(() => lecteur.jouer(blocStereo(48))).not.toThrow();
    expect(lecteur.blocs).toBe(0);
    ContexteFactice.refuser = false;
    lecteur.jouer(new ArrayBuffer(3));
    expect([lecteur.blocs, ContexteFactice.instances]).toEqual([0, []]);
  });
});
