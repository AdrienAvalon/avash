// Son du bureau distant : les blocs PCM que le processus relaie (message
// [20]) sont joués par WebAudio, bout à bout, sur un curseur de temps qui
// avance avec eux. Le volume que le serveur demande ([21]) passe par un gain.
// Rien n'est décodé ici : le processus n'annonce que du PCM 16 bits.

/** En-tête d'un bloc [20] : ce qu'il faut pour le jouer. */
export type BlocPcm = { cadence: number; canaux: number; bits: number; pcm: DataView };

/** Découpe un message [20] : `[20][format u8][ts u32][cadence u32][canaux u8][bits u8][pcm…]`. */
export function decoderBloc(buf: ArrayBuffer): BlocPcm | null {
  if (buf.byteLength < 12) return null;
  const dv = new DataView(buf);
  const cadence = dv.getUint32(6, true);
  const canaux = dv.getUint8(10);
  const bits = dv.getUint8(11);
  if (cadence < 8000 || cadence > 192000 || canaux < 1 || canaux > 2 || bits !== 16) return null;
  const octets = buf.byteLength - 12;
  if (octets === 0 || octets % (canaux * 2) !== 0) return null;
  return { cadence, canaux, bits, pcm: new DataView(buf, 12, octets) };
}

/** PCM 16 bits entrelacé → un tableau de flottants [-1, 1] par canal. */
export function pcm16VersFlottants(pcm: DataView, canaux: number): Float32Array<ArrayBuffer>[] {
  const trames = Math.floor(pcm.byteLength / (2 * canaux));
  const sortie = Array.from({ length: canaux }, () => new Float32Array(new ArrayBuffer(trames * 4)));
  for (let i = 0; i < trames; i++) {
    for (let c = 0; c < canaux; c++) {
      sortie[c][i] = pcm.getInt16((i * canaux + c) * 2, true) / 32768;
    }
  }
  return sortie;
}

/** Le volume RDP (0 à 65535 par canal) en gain linéaire moyen. */
export function gainDepuisVolume(gauche: number, droit: number): number {
  return Math.max(0, Math.min(1, (gauche + droit) / 2 / 65535));
}

/** Combien de blocs peuvent attendre devant le curseur : au-delà, on rattrape
 *  le retard (un serveur qui pousse plus vite que le temps réel, ou un onglet
 *  resté caché) en repartant du présent. */
const AVANCE_MAX_S = 0.5;

export class LecteurAudio {
  private ctx: AudioContext | null = null;
  private gain: GainNode | null = null;
  private curseur = 0;
  /** Blocs joués et échantillons reçus, pour le diagnostic et les tests. */
  blocs = 0;
  echantillons = 0;

  private contexte(cadence: number): { ctx: AudioContext; gain: GainNode } | null {
    if (!this.ctx) {
      try {
        this.ctx = new AudioContext({ sampleRate: cadence });
        this.gain = this.ctx.createGain();
        this.gain.connect(this.ctx.destination);
      } catch {
        return null; // pas d'audio sur cette machine : on ne bloque rien
      }
    }
    if (this.ctx.state === "suspended") void this.ctx.resume().catch(() => {});
    return this.gain ? { ctx: this.ctx, gain: this.gain } : null;
  }

  /** Joue un bloc [20] à la suite des précédents. */
  jouer(buf: ArrayBuffer): void {
    const bloc = decoderBloc(buf);
    if (!bloc) return;
    const c = this.contexte(bloc.cadence);
    if (!c) return;
    const canaux = pcm16VersFlottants(bloc.pcm, bloc.canaux);
    const trames = canaux[0].length;
    const tampon = c.ctx.createBuffer(bloc.canaux, trames, bloc.cadence);
    canaux.forEach((d, i) => tampon.copyToChannel(d, i));
    const source = c.ctx.createBufferSource();
    source.buffer = tampon;
    source.connect(c.gain);
    const maintenant = c.ctx.currentTime;
    // Un peu d'avance pour absorber la gigue ; si le curseur a pris trop
    // d'avance ou est resté en arrière, on repart du présent.
    if (this.curseur < maintenant || this.curseur > maintenant + AVANCE_MAX_S) this.curseur = maintenant + 0.05;
    source.start(this.curseur);
    this.curseur += trames / bloc.cadence;
    this.blocs += 1;
    this.echantillons += trames;
  }

  /** Volume demandé par le serveur (message [21]). */
  volume(gauche: number, droit: number): void {
    if (this.gain) this.gain.gain.value = gainDepuisVolume(gauche, droit);
  }

  fermer(): void {
    void this.ctx?.close().catch(() => {});
    this.ctx = null;
    this.gain = null;
  }
}
