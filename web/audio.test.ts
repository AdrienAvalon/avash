import { describe, expect, it } from "vitest";
import { decoderBloc, gainDepuisVolume, pcm16VersFlottants } from "./audio";

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
