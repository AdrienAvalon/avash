//! Enregistrement et rejeu du dialogue d'un serveur RDP.
//!
//! # Pourquoi
//!
//! Les défauts RDP de ce projet ont tous une chose en commun : ils ne vivaient
//! que dans le dialogue avec un vrai serveur. Un xrdp qui complète ses tuiles à
//! un multiple de quatre, un serveur qui attend des résultats de mesure de bande
//! passante — aucun simulacre ne les aurait produits, parce qu'on ne simule que
//! ce qu'on a déjà compris.
//!
//! Ce module capture ce dialogue une fois, puis le rejoue sans réseau. Trois
//! conséquences :
//!
//! 1. **Un serveur devient une fixture permanente.** Le comportement singulier
//!    d'une machine du parc reste éprouvé même quand la machine a disparu.
//! 2. **Les tests deviennent hermétiques et instantanés.** Pas de conteneur, pas
//!    de TLS, pas de NLA : quelques millisecondes.
//! 3. **Le fuzzing part de trafic réel.** Muter des octets aléatoires ne franchit
//!    jamais les premières validations ; muter un enregistrement authentique
//!    atteint le décodeur d'images, là où les vrais défauts se cachent.
//!
//! Ce troisième point est le plus important pour la sécurité : un serveur RDP
//! est une **entrée non fiable**. Un serveur malveillant, ou simplement abîmé,
//! ne doit pas pouvoir faire tomber le client.
//!
//! # Format
//!
//! ```text
//! "AVASHREC" version:u8
//! largeur:u16 hauteur:u16 io:u16 utilisateur:u16 message:u16 partage:u32 compression:u8
//! canal_dvc:u16 canal_clip:u16
//! puis, répété : action:u8 longueur:u32 charge[longueur]
//! ```
//!
//! Tout en petit-boutien. `message` vaut `0xFFFF` quand aucun canal de message
//! n'a été négocié — un identifiant MCS valide ne prend jamais cette valeur.

use anyhow::{Context as _, Result};
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::Action;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStageBuilder, ActiveStageOutput};
use std::io::{Read as _, Write as _};

const MAGIE: &[u8; 8] = b"AVASHREC";
const VERSION: u8 = 2;
const SANS_CANAL_MESSAGE: u16 = 0xFFFF;

/// Plafond par défaut d'un enregistrement. Une fixture doit tenir dans un
/// dépôt : au-delà, on cesse d'enregistrer plutôt que de gonfler le fichier.
pub const PLAFOND_DEFAUT: u64 = 4 * 1024 * 1024;

/// Paramètres de session nécessaires pour reconstruire l'étape active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entete {
    pub largeur: u16,
    pub hauteur: u16,
    pub io: u16,
    pub utilisateur: u16,
    pub message: Option<u16>,
    pub partage: u32,
    pub compression: u8,
    /// Identifiants MCS des deux canaux statiques que le sidecar enregistre
    /// toujours. Ils sont attribués pendant la connexion, donc invisibles au
    /// rejeu si on ne les capture pas — et sans eux, le rejeu refuse tout PDU
    /// qui leur est adressé.
    pub canal_dvc: u16,
    pub canal_clip: u16,
}

impl Entete {
    fn ecrire(&self, s: &mut Vec<u8>) {
        s.extend_from_slice(&self.largeur.to_le_bytes());
        s.extend_from_slice(&self.hauteur.to_le_bytes());
        s.extend_from_slice(&self.io.to_le_bytes());
        s.extend_from_slice(&self.utilisateur.to_le_bytes());
        s.extend_from_slice(&self.message.unwrap_or(SANS_CANAL_MESSAGE).to_le_bytes());
        s.extend_from_slice(&self.partage.to_le_bytes());
        s.push(self.compression);
        s.extend_from_slice(&self.canal_dvc.to_le_bytes());
        s.extend_from_slice(&self.canal_clip.to_le_bytes());
    }

    fn lire(o: &[u8]) -> Result<Self> {
        anyhow::ensure!(o.len() >= 19, "en-tête tronqué");
        let u16le = |i: usize| u16::from_le_bytes([o[i], o[i + 1]]);
        let message = u16le(8);
        Ok(Self {
            largeur: u16le(0),
            hauteur: u16le(2),
            io: u16le(4),
            utilisateur: u16le(6),
            message: (message != SANS_CANAL_MESSAGE).then_some(message),
            partage: u32::from_le_bytes([o[10], o[11], o[12], o[13]]),
            compression: o[14],
            canal_dvc: u16le(15),
            canal_clip: u16le(17),
        })
    }
}

/// Écrit un enregistrement au fil de la session.
pub struct Enregistreur {
    fichier: std::fs::File,
    ecrit: u64,
    plafond: u64,
    /// Vrai une fois le plafond atteint : on le signale une seule fois.
    sature: bool,
}

impl Enregistreur {
    pub fn nouveau(chemin: &str, entete: &Entete, plafond: u64) -> Result<Self> {
        let mut debut = Vec::with_capacity(32);
        debut.extend_from_slice(MAGIE);
        debut.push(VERSION);
        entete.ecrire(&mut debut);
        let mut fichier =
            std::fs::File::create(chemin).with_context(|| format!("création de {chemin}"))?;
        fichier.write_all(&debut)?;
        Ok(Self {
            fichier,
            ecrit: debut.len() as u64,
            plafond,
            sature: false,
        })
    }

    /// Ajoute un PDU. Au-delà du plafond, on s'arrête sans rien casser : un
    /// enregistrement tronqué reste rejouable, il est simplement plus court.
    pub fn ajouter(&mut self, action: Action, charge: &[u8]) {
        if self.ecrit + charge.len() as u64 + 5 > self.plafond {
            if !self.sature {
                self.sature = true;
                eprintln!("enregistrement : plafond atteint, arrêt de la capture");
            }
            return;
        }
        let code = match action {
            Action::FastPath => 0u8,
            Action::X224 => 1u8,
        };
        let mut tete = [0u8; 5];
        tete[0] = code;
        tete[1..].copy_from_slice(&u32::try_from(charge.len()).unwrap_or(0).to_le_bytes());
        if self.fichier.write_all(&tete).is_ok() && self.fichier.write_all(charge).is_ok() {
            self.ecrit += 5 + charge.len() as u64;
        }
    }
}

/// Un enregistrement lu depuis un fichier.
pub struct Enregistrement {
    pub entete: Entete,
    pub pdus: Vec<(Action, Vec<u8>)>,
}

/// Lit un enregistrement. Un fichier abîmé donne une erreur, jamais une panique
/// ni une allocation démesurée : la longueur annoncée est bornée par ce qui
/// reste réellement dans le fichier.
pub fn lire(chemin: &str) -> Result<Enregistrement> {
    let mut o = Vec::new();
    std::fs::File::open(chemin)
        .with_context(|| format!("ouverture de {chemin}"))?
        .read_to_end(&mut o)?;
    anyhow::ensure!(o.len() >= 9, "fichier trop court");
    anyhow::ensure!(&o[..8] == MAGIE, "ce n'est pas un enregistrement avash");
    anyhow::ensure!(
        o[8] == VERSION,
        "version d'enregistrement inconnue : {}",
        o[8]
    );
    let entete = Entete::lire(&o[9..])?;

    let mut pdus = Vec::new();
    let mut i = 9 + 19;
    while i + 5 <= o.len() {
        let action = match o[i] {
            0 => Action::FastPath,
            1 => Action::X224,
            autre => anyhow::bail!("action inconnue dans l'enregistrement : {autre}"),
        };
        let n = u32::from_le_bytes([o[i + 1], o[i + 2], o[i + 3], o[i + 4]]) as usize;
        i += 5;
        // Borner par ce qui reste : une longueur mensongère ne doit pas faire
        // allouer un gigaoctet ni déborder.
        let fin = i.saturating_add(n).min(o.len());
        pdus.push((action, o[i..fin].to_vec()));
        i = fin;
    }
    Ok(Enregistrement { entete, pdus })
}

/// Ce qu'a produit un rejeu.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resume {
    /// PDU acceptés sans erreur.
    pub acceptes: usize,
    /// PDU FastPath refusés. Sur un enregistrement sain, ce doit être zéro :
    /// c'est le chemin graphique, celui qui porte toutes les images.
    pub refuses: usize,
    /// PDU de canaux statiques refusés, hors périmètre du rejeu.
    ///
    /// Ces canaux — presse-papiers, canal dynamique — ont négocié leurs
    /// capacités AVANT le début de l'enregistrement, qui commence après
    /// l'activation. Rejouer leur conversation à partir du milieu ne peut pas
    /// aboutir, et ce n'est pas ce qu'on cherche : la valeur du rejeu est le
    /// décodage d'images. On les compte, on ne s'en alarme pas.
    pub hors_perimetre: usize,
    /// Mises à jour d'image produites.
    pub rectangles: usize,
    /// Empreinte de l'image finale : deux rejeux du même enregistrement doivent
    /// donner exactement la même. C'est ce qui rend le test « en or » possible.
    pub empreinte: u64,
}

/// Presse-papiers muet, pour le rejeu.
///
/// Les canaux statiques doivent être enregistrés dans le MÊME ordre que dans la
/// session réelle : leurs identifiants MCS en découlent (1004, 1005). Sans eux,
/// le rejeu refuse les PDU adressés à ces canaux et ne ressemble plus à ce que
/// le client a réellement vécu.
///
/// Ce backend ne fait rien : rejouer ne doit toucher ni au presse-papiers du
/// poste, ni à quoi que ce soit d'extérieur.
#[derive(Debug)]
struct PressePapiersMuet;

ironrdp::core::impl_as_any!(PressePapiersMuet);

impl ironrdp::cliprdr::backend::CliprdrBackend for PressePapiersMuet {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "."
    }
    fn client_capabilities(&self) -> ironrdp::cliprdr::pdu::ClipboardGeneralCapabilityFlags {
        ironrdp::cliprdr::pdu::ClipboardGeneralCapabilityFlags::empty()
    }
    fn on_ready(&mut self) {}
    fn on_request_format_list(&mut self) {}
    fn on_process_negotiated_capabilities(
        &mut self,
        _c: ironrdp::cliprdr::pdu::ClipboardGeneralCapabilityFlags,
    ) {
    }
    fn on_remote_copy(&mut self, _f: &[ironrdp::cliprdr::pdu::ClipboardFormat]) {}
    fn on_format_data_request(&mut self, _r: ironrdp::cliprdr::pdu::FormatDataRequest) {}
    fn on_format_data_response(&mut self, _r: ironrdp::cliprdr::pdu::FormatDataResponse<'_>) {}
    fn on_file_contents_request(&mut self, _r: ironrdp::cliprdr::pdu::FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _r: ironrdp::cliprdr::pdu::FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _i: ironrdp::cliprdr::pdu::LockDataId) {}
    fn on_unlock(&mut self, _i: ironrdp::cliprdr::pdu::LockDataId) {}
}

/// Rejoue un enregistrement, sans réseau.
///
/// `tolerant` : continuer après un PDU FastPath refusé. C'est ce que fait le
/// fuzzing, qui mute exprès les octets ; un rejeu de vérification, lui, exige
/// que tout le chemin graphique passe.
pub fn rejouer(e: &Enregistrement, tolerant: bool) -> Result<Resume> {
    let mut image = DecodedImage::new(PixelFormat::RgbA32, e.entete.largeur, e.entete.hauteur);
    // Mêmes canaux, même ordre que la session réelle : drdynvc puis cliprdr.
    let mut canaux = ironrdp::svc::StaticChannelSet::new();
    // Le canal graphique en fait partie : c'est le seul chemin par lequel GNOME
    // Remote Desktop dessine, et sans lui un enregistrement de ce serveur
    // rejouerait sur un écran noir sans que rien ne le signale.
    let (egfx, _canal, file) = crate::egfx::Egfx::nouveau();
    canaux.insert(ironrdp::dvc::DrdynvcClient::new().with_dynamic_channel(egfx));
    canaux.insert(ironrdp::cliprdr::CliprdrClient::new(Box::new(
        PressePapiersMuet,
    )));
    // Réattacher les identifiants capturés : sans eux, tout PDU adressé à ces
    // canaux est refusé et le rejeu ne ressemble plus à la session réelle.
    canaux.attach_channel_id(
        std::any::TypeId::of::<ironrdp::dvc::DrdynvcClient>(),
        e.entete.canal_dvc,
    );
    canaux.attach_channel_id(
        std::any::TypeId::of::<ironrdp::cliprdr::CliprdrClient>(),
        e.entete.canal_clip,
    );
    let mut active = ActiveStageBuilder {
        static_channels: canaux,
        user_channel_id: e.entete.utilisateur,
        io_channel_id: e.entete.io,
        message_channel_id: e.entete.message,
        share_id: e.entete.partage,
        compression_type: None,
        enable_server_pointer: false,
        pointer_software_rendering: true,
    }
    .build();

    let mut r = Resume::default();
    for (action, charge) in &e.pdus {
        match active.process(&mut image, *action, charge) {
            Ok(sorties) => {
                r.acceptes += 1;
                for s in sorties {
                    if matches!(s, ActiveStageOutput::GraphicsUpdate(_)) {
                        r.rectangles += 1;
                    }
                }
                for t in std::mem::take(&mut *file.lock().unwrap()).trames {
                    image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
                    r.rectangles += 1;
                }
            }
            Err(err) => match action {
                Action::X224 => r.hors_perimetre += 1,
                Action::FastPath => {
                    r.refuses += 1;
                    if !tolerant {
                        return Err(anyhow::anyhow!("PDU graphique refusé au rejeu : {err}"));
                    }
                }
            },
        }
    }
    r.empreinte = empreinte(image.data());
    Ok(r)
}

/// Empreinte FNV-1a : ni cryptographique ni destinée à l'être, seulement
/// stable et rapide. Elle sert à comparer deux rendus, pas à résister à qui
/// que ce soit.
#[must_use]
pub fn empreinte(octets: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for o in octets {
        h ^= u64::from(*o);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::{empreinte, lire, rejouer, Enregistrement, Enregistreur, Entete};
    use ironrdp::pdu::Action;

    fn entete() -> Entete {
        Entete {
            largeur: 64,
            hauteur: 32,
            io: 1003,
            utilisateur: 1007,
            message: Some(1006),
            partage: 0x0001_0001,
            compression: 0,
            canal_dvc: 1004,
            canal_clip: 1005,
        }
    }

    fn chemin(nom: &str) -> String {
        std::env::temp_dir()
            .join(format!("avash-rec-{}-{nom}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn un_enregistrement_se_relit_a_l_identique() {
        let p = chemin("aller-retour");
        let mut e = Enregistreur::nouveau(&p, &entete(), 1 << 20).unwrap();
        e.ajouter(Action::FastPath, &[1, 2, 3]);
        e.ajouter(Action::X224, &[9; 40]);
        drop(e);
        let lu = lire(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(lu.entete, entete());
        assert_eq!(lu.pdus.len(), 2);
        assert_eq!(lu.pdus[0].1, vec![1, 2, 3]);
        assert_eq!(lu.pdus[1].1.len(), 40);
    }

    #[test]
    fn le_plafond_arrete_la_capture_sans_casser_le_fichier() {
        let p = chemin("plafond");
        // Plafond volontairement minuscule : la première charge passe, pas la
        // suite. 28 = 8 (magie) + 1 (version) + 19 (en-tête).
        let mut e = Enregistreur::nouveau(&p, &entete(), 28 + 5 + 4).unwrap();
        e.ajouter(Action::FastPath, &[7; 4]);
        for _ in 0..50 {
            e.ajouter(Action::FastPath, &[7; 4]);
        }
        drop(e);
        let lu = lire(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(lu.pdus.len(), 1, "le plafond doit borner la capture");
        assert_eq!(lu.entete, entete(), "l'en-tête reste lisible");
    }

    #[test]
    fn une_longueur_mensongere_ne_fait_pas_exploser_la_lecture() {
        // Le piège : un fichier annonçant un PDU d'un gigaoctet. Sans borne, la
        // lecture allouerait à l'aveugle.
        let p = chemin("mensonge");
        let mut o = b"AVASHREC".to_vec();
        o.push(super::VERSION);
        o.extend_from_slice(&[0u8; 19]);
        o.push(0);
        o.extend_from_slice(&u32::MAX.to_le_bytes());
        o.extend_from_slice(&[42u8; 8]);
        std::fs::write(&p, &o).unwrap();
        let lu = lire(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(lu.pdus.len(), 1);
        assert_eq!(lu.pdus[0].1.len(), 8, "borné par ce qui reste réellement");
    }

    #[test]
    fn un_fichier_etranger_est_refuse() {
        let p = chemin("etranger");
        std::fs::write(&p, b"ceci n'est pas un enregistrement").unwrap();
        let r = lire(&p);
        std::fs::remove_file(&p).ok();
        assert!(r.is_err());
    }

    #[test]
    fn le_rejeu_d_un_enregistrement_vide_est_stable() {
        let e = Enregistrement {
            entete: entete(),
            pdus: Vec::new(),
        };
        let a = rejouer(&e, false).unwrap();
        let b = rejouer(&e, false).unwrap();
        assert_eq!(a, b, "deux rejeux identiques doivent donner le même résumé");
    }

    #[test]
    fn l_empreinte_distingue_deux_images() {
        assert_ne!(empreinte(&[1, 2, 3]), empreinte(&[1, 2, 4]));
        assert_eq!(empreinte(&[1, 2, 3]), empreinte(&[1, 2, 3]));
    }
}

/// Tests sur enregistrements réels du parc.
///
/// Ces fixtures sont le dialogue authentique de serveurs xrdp — XFCE et GNOME —
/// capturé une fois puis figé. Elles rejouent en cinq millisecondes ce qui
/// demande cinq secondes de connexion : mille fois plus vite, sans conteneur,
/// sans TLS, sans NLA.
#[cfg(test)]
mod tests_enregistrements_reels {
    use super::{lire, rejouer};

    /// Empreintes de référence. Elles changent si le décodage change — c'est
    /// précisément ce qu'on veut : une modification du rendu doit être une
    /// décision, jamais un effet de bord.
    ///
    /// Vérifié : en débranchant le correctif du remplissage des tuiles, l'empreinte
    /// de `xfce` passe de df04a5d714c2a784 à 3a5ac9ea470a6a13. Ce test voit donc
    /// le cisaillement — celui qu'il a fallu un signalement d'utilisateur pour
    /// découvrir — sans réseau ni serveur.
    const ATTENDUES: &[(&str, u64)] = &[
        ("xfce", 0xdf04_a5d7_14c2_a784),
        ("gnome", 0xe260_e3e2_7f26_bdf4),
        // GNOME Remote Desktop : tout passe par le canal graphique et le codec
        // RemoteFX Progressive, en tuiles simples.
        //
        // Windows emprunte le même canal mais rien d'autre du même chemin :
        // ClearCodec, cache de surfaces, remplissages unis, et un progressif
        // affiné par paliers de qualité. Deux enregistrements, deux moitiés
        // disjointes du décodeur — et le parc conteneurisé n'en héberge aucun
        // des deux.
        ("gnome-remote-desktop", 0x44fd_e714_c1d2_750e),
        ("windows-egfx", 0x92dc_087e_677a_53d4),
    ];

    fn chemin(nom: &str) -> String {
        format!(
            "{}/../tests-parc/enregistrements/{nom}.rec",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn le_rendu_des_enregistrements_reels_ne_bouge_pas() {
        for (nom, attendue) in ATTENDUES {
            let e = lire(&chemin(nom)).expect("enregistrement lisible");
            let r = rejouer(&e, false).expect("rejeu sans refus graphique");
            assert_eq!(r.refuses, 0, "{nom} : un PDU graphique a été refusé");
            assert!(r.rectangles > 0, "{nom} : aucune image produite");
            assert_eq!(
                r.empreinte, *attendue,
                "{nom} : le rendu a changé. Si c'est voulu, mettre à jour \
                 l'empreinte ; sinon, c'est une régression du décodage."
            );
        }
    }

    #[test]
    fn deux_rejeux_donnent_exactement_le_meme_resultat() {
        // Sans déterminisme, l'empreinte ne prouverait rien.
        let e = lire(&chemin("xfce")).unwrap();
        assert_eq!(rejouer(&e, false).unwrap(), rejouer(&e, false).unwrap());
    }

    /// Fuzzing par mutation, à partir de trafic RÉEL.
    ///
    /// Un serveur RDP est une entrée non fiable : rien n'oblige celui d'en face
    /// à être bienveillant, ni même correct. Muter des octets au hasard ne
    /// mène nulle part — les premières validations les rejettent toutes. Muter
    /// un enregistrement authentique, en revanche, atteint le décodeur
    /// d'images, là où vivent les vrais défauts.
    #[test]
    fn un_serveur_hostile_ne_fait_pas_tomber_le_client() {
        // Les deux chemins de décodage, car ils ne partagent presque rien :
        // « xfce » emprunte les mises à jour classiques ; « gnome-remote-desktop »
        // le canal graphique et RemoteFX Progressive en tuiles simples ;
        // « windows-egfx » ClearCodec, le cache de surfaces et le progressif
        // affiné par paliers. Chacun lit des longueurs, des indices de tuile et
        // des emplacements de cache fournis par le serveur.
        let bases: Vec<_> = ["xfce", "gnome-remote-desktop", "windows-egfx"]
            .iter()
            .map(|n| lire(&chemin(n)).unwrap())
            .collect();
        let mut graine: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut alea = || {
            graine ^= graine << 13;
            graine ^= graine >> 7;
            graine ^= graine << 17;
            graine
        };
        let mut refuses_total = 0usize;
        // Campagne courte par défaut pour ne pas alourdir chaque vérification ;
        // AVASH_FUZZ_TOURS permet d'en lancer une longue à la demande.
        let tours: usize = std::env::var("AVASH_FUZZ_TOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        for tour in 0..tours {
            let base = &bases[tour % bases.len()];
            let mut mute = super::Enregistrement {
                entete: base.entete,
                pdus: base.pdus.clone(),
            };
            // Une poignée d'octets retournés : assez pour dérouter le décodeur,
            // pas assez pour que tout soit rejeté d'emblée.
            for _ in 0..8 {
                if mute.pdus.is_empty() {
                    break;
                }
                let i = (alea() as usize) % mute.pdus.len();
                let charge = &mut mute.pdus[i].1;
                if charge.is_empty() {
                    continue;
                }
                let j = (alea() as usize) % charge.len();
                charge[j] ^= (alea() & 0xff) as u8;
            }
            // Tolérant : on s'attend à des refus. Ce qu'on n'accepte pas, c'est
            // une panique — elle ferait tomber une session déjà établie.
            let r = rejouer(&mute, true).expect("le rejeu tolérant ne doit jamais échouer");
            refuses_total += r.refuses;
        }
        // Sans cette assertion, un fuzzing qui n'atteindrait rien passerait pour
        // un succès. Les mutations doivent réellement déranger le décodeur.
        assert!(
            refuses_total > 0,
            "aucune mutation n'a été refusée : elles n'atteignent pas le décodeur"
        );
    }
}
