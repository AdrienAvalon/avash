//! Entrées de l'interface vers le serveur : souris, clavier, verrous.

use ironrdp::input::{MousePosition, Operation, WheelRotations};

pub(crate) fn mouse_button(n: u8) -> ironrdp::input::MouseButton {
    use ironrdp::input::MouseButton::{Left, Middle, Right, X1, X2};
    match n {
        1 => Middle,
        2 => Right,
        3 => X1,
        4 => X2,
        _ => Left,
    }
}

/// Aligne les verrous clavier du bureau distant sur ceux du poste.
///
/// Sans cet événement, la session distante démarre avec ses propres verrous :
/// le pavé numérique paraît inactif alors qu'il est allumé côté utilisateur,
/// qui doit appuyer sur Verr.Num pour « resynchroniser » les deux.
///
/// Bits du message [10] : 1 = numérique, 2 = majuscules, 4 = défilement.
pub(crate) fn lock_sync_event(bits: u8) -> ironrdp::pdu::input::fast_path::FastPathInputEvent {
    ironrdp::input::synchronize_event(
        bits & 0b100 != 0, // défilement
        bits & 0b001 != 0, // numérique
        bits & 0b010 != 0, // majuscules
        false,             // kana : claviers japonais, non géré
    )
}

/// Décode un message d'entrée binaire en opérations IronRDP.
pub(crate) fn input_ops(b: &[u8]) -> Vec<Operation> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    match b.first().copied() {
        Some(1) if b.len() >= 5 => vec![Operation::MouseMove(MousePosition {
            x: u16le(1),
            y: u16le(3),
        })],
        Some(2) if b.len() >= 7 => {
            let bt = mouse_button(b[1]);
            let click = if b[2] != 0 {
                Operation::MouseButtonPressed(bt)
            } else {
                Operation::MouseButtonReleased(bt)
            };
            vec![
                Operation::MouseMove(MousePosition {
                    x: u16le(3),
                    y: u16le(5),
                }),
                click,
            ]
        }
        Some(3) if b.len() >= 3 => {
            let d = i16::from_le_bytes([b[1], b[2]]);
            vec![Operation::WheelRotations(WheelRotations {
                is_vertical: true,
                rotation_units: d,
            })]
        }
        Some(4) if b.len() >= 4 => {
            let sc = ironrdp::input::Scancode::from(u16le(1));
            vec![if b[3] != 0 {
                Operation::KeyPressed(sc)
            } else {
                Operation::KeyReleased(sc)
            }]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests_entrees_hostiles {
    use super::input_ops;

    /// Générateur déterministe : un échec doit pouvoir être rejoué à
    /// l'identique. Un test aléatoire non reproductible n'aide personne.
    fn suite(graine: u64) -> impl FnMut() -> u64 {
        let mut e = graine;
        move || {
            e ^= e << 13;
            e ^= e >> 7;
            e ^= e << 17;
            e
        }
    }

    #[test]
    fn aucun_message_malforme_ne_fait_paniquer() {
        // Ces octets viennent du canal local. Il est authentifié par jeton, mais
        // un client authentifié reste un client : rien ne garantit qu'il envoie
        // des messages bien formés — un bogue d'interface suffit. Une analyse
        // qui panique ferait tomber une session RDP déjà établie.
        let mut alea = suite(0x5eed_1234_abcd_ef01);
        for _ in 0..20_000 {
            let n = (alea() % 24) as usize;
            let mut b = Vec::with_capacity(n);
            for _ in 0..n {
                b.push((alea() & 0xff) as u8);
            }
            let _ = input_ops(&b);
        }
    }

    #[test]
    fn chaque_type_connu_tronque_a_toutes_les_longueurs() {
        // Le vrai piège n'est pas l'octet aléatoire mais le message VALIDE
        // coupé trop tôt : le type est reconnu, la charge manque.
        for type_msg in 0u8..=13 {
            for longueur in 0..20usize {
                let mut b = vec![type_msg];
                b.extend(std::iter::repeat_n(0xa5u8, longueur));
                let _ = input_ops(&b);
            }
        }
    }

    #[test]
    fn un_message_vide_ne_produit_rien() {
        assert!(input_ops(&[]).is_empty());
    }
}
