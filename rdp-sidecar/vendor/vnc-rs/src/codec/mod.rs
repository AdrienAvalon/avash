mod cursor;
mod raw;
mod tight;
mod trle;
mod zlib;
mod zrle;
pub(crate) use cursor::Decoder as CursorDecoder;
pub(crate) use raw::Decoder as RawDecoder;
pub(crate) use tight::Decoder as TightDecoder;
pub(crate) use trle::Decoder as TrleDecoder;
pub(crate) use zrle::Decoder as ZrleDecoder;

/// Plus grand tampon qu'un décodeur accepte d'allouer d'un coup : un cadre de
/// 8192 × 8192 pixels sur quatre octets, la borne que le client impose aussi à
/// la résolution annoncée. Un serveur reste une entrée non fiable : une
/// longueur de 32 bits lue sur le fil (ZRLE, Tight) ou un rectangle de
/// 65535 × 65535 faisaient allouer jusqu'à 17 Gio avant de lire quoi que ce
/// soit — le processus mourait par manque de mémoire, et une session ouverte
/// avec lui.
pub(crate) const TAMPON_MAX: usize = 8192 * 8192 * 4;

/// Un tampon de `len` octets, mis à zéro, ou une erreur si la taille dépasse
/// [`TAMPON_MAX`]. Remplace un `Vec` non initialisé (`set_len` sur une
/// capacité) : lire dans une mémoire non initialisée est un comportement
/// indéfini en Rust, et la mise à zéro ne coûte rien face au réseau.
pub(crate) fn tampon(len: usize) -> Result<Vec<u8>, crate::VncError> {
    if len > TAMPON_MAX {
        return Err(crate::VncError::General(format!(
            "le serveur annonce {len} octets d'un coup, plus que la borne de {TAMPON_MAX}"
        )));
    }
    Ok(vec![0; len])
}

#[cfg(test)]
mod tests_tampon {
    use super::{tampon, TAMPON_MAX};

    /// Trouvé en relisant les décodeurs avant de les embarquer : chaque
    /// longueur lue sur le fil devenait une allocation, sans borne.
    #[test]
    fn une_longueur_deraisonnable_est_refusee_sans_allouer() {
        assert!(tampon(TAMPON_MAX + 1).is_err());
        assert!(tampon(usize::MAX).is_err());
        assert!(tampon(u32::MAX as usize).is_err());
    }

    #[test]
    fn un_tampon_ordinaire_est_a_zero() {
        let t = tampon(16).unwrap();
        assert_eq!(t.len(), 16);
        assert!(t.iter().all(|&o| o == 0));
        assert!(tampon(0).unwrap().is_empty());
        assert_eq!(tampon(TAMPON_MAX).unwrap().len(), TAMPON_MAX);
    }
}
