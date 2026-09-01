//! Lecture d'un PDU de redirection de serveur (MS-RDPBCGR 2.2.13.1.1).
//!
//! IronRDP connaît le *type* `ServerRedirect` (0x0A) mais ne sait pas le
//! décoder : `ShareControlPdu::from_type` le rejette. Or c'est exactement ce
//! qu'envoie GNOME Remote Desktop une fois la session ouverte, pour renvoyer le
//! client vers la session de l'utilisateur. Sans ce décodage, la connexion
//! s'arrête sur « unexpected share control PDU type ».

/// Ce qu'un serveur nous demande de faire.
#[derive(Debug, Default, Clone)]
pub struct Redirection {
    pub session_id: u32,
    pub drapeaux: u32,
    /// Adresse de destination, si le serveur en désigne une.
    pub adresse: Option<String>,
    /// Jeton de routage à replacer dans la requête de connexion X.224.
    pub jeton: Option<Vec<u8>>,
    pub utilisateur: Option<String>,
    pub domaine: Option<String>,
    /// Mot de passe fourni par le serveur. **Jamais journalisé.**
    pub mot_de_passe: Option<Vec<u8>>,
    pub fqdn: Option<String>,
    /// Identifiant unique de la connexion redirigée.
    pub guid: Option<Vec<u8>>,
    /// Les mêmes champs, **tels quels**. RDSTLS les réémet octet pour octet —
    /// UTF-16 et terminateur nul compris — et non sous leur forme décodée.
    pub utilisateur_brut: Option<Vec<u8>>,
    pub domaine_brut: Option<Vec<u8>>,
}

const LB_TARGET_NET_ADDRESS: u32 = 0x0000_0001;
const LB_LOAD_BALANCE_INFO: u32 = 0x0000_0002;
const LB_USERNAME: u32 = 0x0000_0004;
const LB_DOMAIN: u32 = 0x0000_0008;
const LB_PASSWORD: u32 = 0x0000_0010;
const LB_TARGET_FQDN: u32 = 0x0000_0100;
const LB_TARGET_NETBIOS_NAME: u32 = 0x0000_0200;
const LB_CLIENT_TSV_URL: u32 = 0x0000_1000;
const LB_REDIRECTION_GUID: u32 = 0x0000_8000;

/// Lit un champ « longueur puis données », en restant dans les bornes.
fn champ<'a>(o: &'a [u8], i: &mut usize) -> Option<&'a [u8]> {
    if *i + 4 > o.len() {
        return None;
    }
    let n = u32::from_le_bytes([o[*i], o[*i + 1], o[*i + 2], o[*i + 3]]) as usize;
    *i += 4;
    // Borner par ce qui reste : une longueur mensongère ne doit pas déborder.
    let fin = i.saturating_add(n).min(o.len());
    let v = &o[*i..fin];
    *i = fin;
    Some(v)
}

/// Décode de l'UTF-16 petit-boutien, en ignorant le terminateur nul.
fn utf16(o: &[u8]) -> String {
    let mots: Vec<u16> = o
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16_lossy(&mots)
}

/// Décode la charge d'un PDU de redirection.
///
/// L'appelant retire les **huit** premiers octets : l'en-tête Share Control en
/// fait six, suivis de deux octets de remplissage propres à ce type de PDU.
/// Vérifié sur les octets d'un vrai serveur — avec six, tous les champs
/// glissaient de deux et le jeton devenait illisible.
///
/// Tolérant par construction : un champ tronqué arrête la lecture au lieu de
/// faire échouer l'ensemble. Ce qui a été lu avant reste exploitable, et c'est
/// ce qui compte — le jeton de routage vient en premier.
#[must_use]
pub fn decoder(charge: &[u8]) -> Option<Redirection> {
    if charge.len() < 12 {
        return None;
    }
    let mut r = Redirection {
        session_id: u32::from_le_bytes([charge[4], charge[5], charge[6], charge[7]]),
        drapeaux: u32::from_le_bytes([charge[8], charge[9], charge[10], charge[11]]),
        ..Redirection::default()
    };
    let mut i = 12;
    if r.drapeaux & LB_TARGET_NET_ADDRESS != 0 {
        r.adresse = champ(charge, &mut i).map(utf16);
    }
    if r.drapeaux & LB_LOAD_BALANCE_INFO != 0 {
        r.jeton = champ(charge, &mut i).map(<[u8]>::to_vec);
    }
    if r.drapeaux & LB_USERNAME != 0 {
        let brut = champ(charge, &mut i).map(<[u8]>::to_vec);
        r.utilisateur = brut.as_deref().map(utf16);
        r.utilisateur_brut = brut;
    }
    if r.drapeaux & LB_DOMAIN != 0 {
        let brut = champ(charge, &mut i).map(<[u8]>::to_vec);
        r.domaine = brut.as_deref().map(utf16);
        r.domaine_brut = brut;
    }
    if r.drapeaux & LB_PASSWORD != 0 {
        r.mot_de_passe = champ(charge, &mut i).map(<[u8]>::to_vec);
    }
    if r.drapeaux & LB_TARGET_FQDN != 0 {
        r.fqdn = champ(charge, &mut i).map(utf16);
    }
    // L'ordre des champs est celui de la spécification, pas celui des drapeaux :
    // sauter un champ présent décalerait tous les suivants.
    if r.drapeaux & LB_TARGET_NETBIOS_NAME != 0 {
        champ(charge, &mut i);
    }
    if r.drapeaux & LB_CLIENT_TSV_URL != 0 {
        champ(charge, &mut i);
    }
    if r.drapeaux & LB_REDIRECTION_GUID != 0 {
        r.guid = champ(charge, &mut i).map(<[u8]>::to_vec);
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::{decoder, utf16};

    fn u16le(s: &str) -> Vec<u8> {
        s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn une_redirection_avec_jeton_et_adresse_est_lue() {
        let adresse = u16le("10.0.0.7");
        let jeton = b"Cookie: msts=1234".to_vec();
        let mut o = vec![0, 0, 0, 0]; // flags + length
        o.extend_from_slice(&7u32.to_le_bytes()); // session id
        o.extend_from_slice(&0x0000_0003u32.to_le_bytes()); // adresse + jeton
        o.extend_from_slice(&(adresse.len() as u32).to_le_bytes());
        o.extend_from_slice(&adresse);
        o.extend_from_slice(&(jeton.len() as u32).to_le_bytes());
        o.extend_from_slice(&jeton);
        let r = decoder(&o).expect("décodable");
        assert_eq!(r.session_id, 7);
        assert_eq!(r.adresse.as_deref(), Some("10.0.0.7"));
        assert_eq!(r.jeton.as_deref(), Some(&jeton[..]));
    }

    #[test]
    fn une_longueur_mensongere_ne_deborde_pas() {
        // Le serveur est une entrée non fiable, y compris ici.
        let mut o = vec![0, 0, 0, 0];
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&0x0000_0001u32.to_le_bytes());
        o.extend_from_slice(&u32::MAX.to_le_bytes());
        o.extend_from_slice(&[0x41, 0x00]);
        let r = decoder(&o).expect("décodable");
        assert_eq!(r.adresse.as_deref(), Some("A"));
    }

    #[test]
    fn une_charge_trop_courte_ne_donne_rien() {
        assert!(decoder(&[1, 2, 3]).is_none());
    }

    #[test]
    fn l_utf16_s_arrete_au_nul() {
        assert_eq!(utf16(&u16le("abc")), "abc");
    }
}
