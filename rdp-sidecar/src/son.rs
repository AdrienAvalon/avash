//! Son du bureau distant (canal statique `rdpsnd`, MS-RDPEA), joué par la
//! webview.
//!
//! Le processus n'ouvre aucun périphérique audio : il n'annonce au serveur que
//! du PCM 16 bits (44,1 et 48 kHz, stéréo et mono), reçoit les blocs d'ondes
//! et les passe tels quels à l'interface (message `[20]`), qui les joue par
//! WebAudio. Pas de bibliothèque audio native à lier, donc rien à embarquer
//! dans l'AppImage ni à réclamer au bac à sable Flatpak ; et un codec (OPUS,
//! AAC) refusé d'emblée reste un codec de moins à faire décoder à un serveur
//! hostile. Le volume que le serveur demande passe aussi (message `[21]`).

use ironrdp::rdpsnd::client::RdpsndClientHandler;
use ironrdp::rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat};
use std::borrow::Cow;

/// Un bloc d'ondes reçu, ou un réglage : ce que la boucle de session relaie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Son {
    Onde {
        /// Indice dans `formats()` : dit la cadence et le nombre de canaux.
        format_no: usize,
        /// Horodatage du serveur (ms), tel quel.
        ts: u32,
        /// Échantillons PCM 16 bits, petit-boutiste, canaux entrelacés.
        pcm: Vec<u8>,
    },
    Volume {
        gauche: u16,
        droit: u16,
    },
}

/// Ce que l'on sait consommer, dans l'ordre de préférence : le serveur choisit
/// le premier format commun. Que du PCM : la webview le joue sans décodage.
pub(crate) fn formats() -> Vec<AudioFormat> {
    fn pcm(hz: u32, canaux: u16) -> AudioFormat {
        let bloc = canaux * 2;
        AudioFormat {
            format: WaveFormat::PCM,
            n_channels: canaux,
            n_samples_per_sec: hz,
            n_avg_bytes_per_sec: hz * u32::from(bloc),
            n_block_align: bloc,
            bits_per_sample: 16,
            data: None,
        }
    }
    vec![
        pcm(44_100, 2),
        pcm(48_000, 2),
        pcm(44_100, 1),
        pcm(48_000, 1),
    ]
}

/// Un bloc plus gros que ça n'est pas de l'audio (une seconde de PCM stéréo à
/// 48 kHz fait 192 Kio) : borné, comme tout ce qui vient du serveur.
pub(crate) const ONDE_MAX: usize = 1 << 20;

/// Le message `[20]` vers l'interface : cadence et canaux en tête, pour qu'elle
/// n'ait pas à retenir la négociation, puis le PCM.
///
/// `[20][format_no u8][ts u32 LE][cadence u32 LE][canaux u8][bits u8][pcm…]`
pub(crate) fn message_onde(
    formats: &[AudioFormat],
    format_no: usize,
    ts: u32,
    pcm: &[u8],
) -> Option<Vec<u8>> {
    let f = formats.get(format_no)?;
    if pcm.is_empty() || pcm.len() > ONDE_MAX {
        return None;
    }
    let mut m = Vec::with_capacity(12 + pcm.len());
    m.push(20u8);
    m.push(u8::try_from(format_no).ok()?);
    m.extend_from_slice(&ts.to_le_bytes());
    m.extend_from_slice(&f.n_samples_per_sec.to_le_bytes());
    m.push(u8::try_from(f.n_channels).ok()?);
    m.push(u8::try_from(f.bits_per_sample).ok()?);
    m.extend_from_slice(pcm);
    Some(m)
}

/// Le message `[21]` : volume demandé par le serveur, deux canaux sur 16 bits.
pub(crate) fn message_volume(gauche: u16, droit: u16) -> Vec<u8> {
    let mut m = vec![21u8];
    m.extend_from_slice(&gauche.to_le_bytes());
    m.extend_from_slice(&droit.to_le_bytes());
    m
}

/// Le gestionnaire que le canal appelle : il ne fait que transmettre.
#[derive(Debug)]
pub(crate) struct SonBackend {
    formats: Vec<AudioFormat>,
    tx: tokio::sync::mpsc::UnboundedSender<Son>,
}

impl SonBackend {
    pub(crate) fn new(tx: tokio::sync::mpsc::UnboundedSender<Son>) -> Self {
        Self {
            formats: formats(),
            tx,
        }
    }

    /// Un canal audio qui n'annonce aucun format et ne se dit pas vivant : le
    /// serveur n'y encode rien. Il n'existe que parce que MS-RDPEFS exige que
    /// `rdpdr` soit annoncé avec `rdpsnd` pour que le serveur lui réponde
    /// (appendice A, note 1) : sans lui, couper le son couperait le lecteur.
    pub(crate) fn muet() -> Self {
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            formats: Vec::new(),
            tx,
        }
    }
}

impl RdpsndClientHandler for SonBackend {
    fn get_flags(&self) -> AudioFormatFlags {
        // ALIVE est obligatoire pour recevoir quoi que ce soit ; VOLUME dit que
        // le réglage du serveur sera appliqué (par la webview).
        if self.formats.is_empty() {
            return AudioFormatFlags::empty();
        }
        AudioFormatFlags::ALIVE | AudioFormatFlags::VOLUME
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn wave(&mut self, format_no: usize, ts: u32, data: Cow<'_, [u8]>) {
        let _ = self.tx.send(Son::Onde {
            format_no,
            ts,
            pcm: data.into_owned(),
        });
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        let _ = self.tx.send(Son::Volume {
            gauche: volume.volume_left,
            droit: volume.volume_right,
        });
    }

    fn set_pitch(&mut self, _pitch: PitchPdu) {
        // Pas de changement de hauteur : le drapeau PITCH n'est pas annoncé.
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Que du PCM 16 bits, stéréo avant mono, 44,1 kHz avant 48 : ce que
    /// Windows et xrdp savent tous deux fournir sans transcodage.
    #[test]
    fn les_formats_annonces_sont_du_pcm_seize_bits() {
        let f = formats();
        assert_eq!(f.len(), 4);
        assert!(f
            .iter()
            .all(|x| x.format == WaveFormat::PCM && x.bits_per_sample == 16));
        assert_eq!((f[0].n_samples_per_sec, f[0].n_channels), (44_100, 2));
        assert_eq!(f[0].n_block_align, 4);
        assert_eq!(f[0].n_avg_bytes_per_sec, 176_400);
        assert_eq!((f[2].n_samples_per_sec, f[2].n_channels), (44_100, 1));
    }

    /// L'en-tête dit la cadence et les canaux du format négocié ; un format
    /// inconnu, un bloc vide ou démesuré ne partent pas.
    #[test]
    fn le_message_porte_cadence_canaux_et_pcm() {
        let f = formats();
        let m = message_onde(&f, 1, 0x0102_0304, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m[0], 20);
        assert_eq!(m[1], 1);
        assert_eq!(u32::from_le_bytes([m[2], m[3], m[4], m[5]]), 0x0102_0304);
        assert_eq!(u32::from_le_bytes([m[6], m[7], m[8], m[9]]), 48_000);
        assert_eq!((m[10], m[11]), (2, 16));
        assert_eq!(&m[12..], &[1, 2, 3, 4]);
        assert!(message_onde(&f, 9, 0, &[1, 2]).is_none(), "format inconnu");
        assert!(message_onde(&f, 0, 0, &[]).is_none(), "bloc vide");
        assert!(
            message_onde(&f, 0, 0, &vec![0u8; ONDE_MAX + 1]).is_none(),
            "bloc démesuré"
        );
        assert_eq!(
            message_volume(0xFFFF, 0x8000),
            vec![21, 0xFF, 0xFF, 0x00, 0x80]
        );
    }

    /// Le gestionnaire transmet ondes et volume, dans l'ordre, sans les
    /// interpréter.
    #[test]
    fn le_gestionnaire_relaie_ondes_et_volume() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b = SonBackend::new(tx);
        assert!(b.get_flags().contains(AudioFormatFlags::ALIVE));
        assert_eq!(b.get_formats().len(), 4);
        b.wave(0, 7, Cow::Borrowed(&[9, 9]));
        b.set_volume(VolumePdu {
            volume_left: 1,
            volume_right: 2,
        });
        assert_eq!(
            rx.try_recv().unwrap(),
            Son::Onde {
                format_no: 0,
                ts: 7,
                pcm: vec![9, 9]
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            Son::Volume {
                gauche: 1,
                droit: 2
            }
        );
        assert!(rx.try_recv().is_err());
    }
}
