//! Un serveur VNC est une entrée non fiable. Ces tests scénarisent un serveur
//! entier dans un tampon (`tokio::io::duplex`) : le client n'a pas besoin de
//! réponses de sa part, il écrit dans le vide et lit ce qui l'attend. Chaque
//! cas vient d'une relecture du code avant de l'embarquer dans avash : une
//! panique (`assert!`, `unimplemented!`), un `transmute` sur une valeur venue
//! du réseau, ou une allocation à la taille que le serveur dicte.

use crate::{PixelFormat, VncConnector, VncEncoding, VncEvent};
use tokio::io::{duplex, AsyncWriteExt, DuplexStream};

const VERSION: &[u8] = b"RFB 003.008\n";

/// Format de pixel « rgba » tel que le serveur l'annonce (16 octets).
const FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0];

/// Écrit d'un bloc tout ce que le serveur dira ; le client est lancé après.
async fn serveur(script: &[u8]) -> DuplexStream {
    let (client, mut serveur) = duplex(1 << 16);
    serveur.write_all(script).await.unwrap();
    // Le côté serveur reste ouvert : l'appelant décide quand fermer.
    std::mem::forget(serveur);
    client
}

/// Même chose, mais le serveur lit `a_lire` octets du client puis raccroche :
/// une raison d'échec se lit jusqu'à la fin du flux, et le client doit avoir
/// pu écrire ses propres messages avant que le tuyau ne casse.
fn serveur_qui_raccroche(script: Vec<u8>, a_lire: usize) -> DuplexStream {
    let (client, mut serveur) = duplex(1 << 16);
    tokio::spawn(async move {
        serveur.write_all(&script).await.unwrap();
        let mut poubelle = vec![0u8; a_lire];
        tokio::io::AsyncReadExt::read_exact(&mut serveur, &mut poubelle)
            .await
            .unwrap();
        drop(serveur);
    });
    client
}

/// Un serveur qui, après l'entrée, annonce un cadre `largeur` × `hauteur`
/// sans authentification.
fn script_sans_auth(largeur: u16, hauteur: u16) -> Vec<u8> {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 1]); // un seul type de sécurité : None
    s.extend_from_slice(&0u32.to_be_bytes()); // SecurityResult : ok
    s.extend_from_slice(&largeur.to_be_bytes());
    s.extend_from_slice(&hauteur.to_be_bytes());
    s.extend_from_slice(&FORMAT);
    s.extend_from_slice(&1u32.to_be_bytes());
    s.push(b't');
    s
}

fn connecteur(
    flux: DuplexStream,
) -> crate::client::connector::VncState<
    DuplexStream,
    impl std::future::Future<Output = Result<String, crate::VncError>> + Send + Sync + 'static,
> {
    VncConnector::new(flux)
        .set_auth_method(async { Ok("secret".to_owned()) })
        .add_encoding(VncEncoding::Raw)
        .set_pixel_format(PixelFormat::rgba())
        .build()
        .unwrap()
}

/// `assert!(!security_types.is_empty())` à l'origine : un serveur qui
/// n'annonce aucun type de sécurité après en avoir promis faisait tomber le
/// client. (Le cas « zéro types » est déjà une erreur en lecture ; celui-ci
/// vérifie que la voie reste une erreur, jamais une panique.)
#[tokio::test]
async fn un_serveur_sans_type_de_securite_donne_une_erreur() {
    let mut s = VERSION.to_vec();
    s.push(0); // aucun type : suivi d'une raison
    s.extend_from_slice(&4u32.to_be_bytes());
    s.extend_from_slice(b"nope");
    // Le serveur lit la version du client (12 octets) puis raccroche : la
    // raison se lit jusqu'à la fin du flux.
    let flux = serveur_qui_raccroche(s, 12);
    let Err(e) = connecteur(flux).try_start().await else {
        panic!("un serveur sans type de sécurité a été accepté")
    };
    assert!(e.to_string().contains("nope"), "{e}");
}

/// `AuthResult::from(u32)` transmutait la valeur du serveur vers une
/// énumération à deux variantes : 7 était un comportement indéfini. C'est un
/// échec d'authentification, dit comme tel.
#[tokio::test]
async fn un_resultat_d_authentification_inconnu_est_un_echec() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 2]); // VncAuth
    s.extend_from_slice(&[0x5a; 16]); // défi
    s.extend_from_slice(&7u32.to_be_bytes()); // ni 0 ni 1
    s.extend_from_slice(&3u32.to_be_bytes());
    s.extend_from_slice(b"bad");
    // Version (12), choix du type (1), réponse au défi (16) : puis le serveur
    // raccroche et la raison se lit jusqu'à la fin du flux.
    let flux = serveur_qui_raccroche(s, 12 + 1 + 16);
    let Err(e) = connecteur(flux).try_start().await else {
        panic!("un résultat d'authentification à 7 a été pris pour un succès")
    };
    assert!(
        matches!(e, crate::VncError::WrongPassword),
        "un refus d'authentification doit se présenter comme tel : {e}"
    );
}

/// rustvncserver raccroche juste après le résultat, sans la raison que la
/// 3.8 prévoit : l'utilisateur voyait « unexpected end of file » au lieu d'un
/// mot de passe refusé (vu par le scénario bout en bout VNC, 2026-09-04).
#[tokio::test]
async fn un_refus_sans_raison_reste_un_mot_de_passe_refuse() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 2]);
    s.extend_from_slice(&[0x5a; 16]);
    s.extend_from_slice(&1u32.to_be_bytes()); // refusé, et plus rien
    let flux = serveur_qui_raccroche(s, 12 + 1 + 16);
    let Err(e) = connecteur(flux).try_start().await else {
        panic!("un refus sans raison a été pris pour un succès")
    };
    assert!(matches!(e, crate::VncError::WrongPassword), "{e}");
}

/// Un cadre de 65535 × 65535 fait 17 Gio en RGBA : refusé à l'entrée, avant
/// que quiconque n'alloue.
#[tokio::test]
async fn une_resolution_deraisonnable_est_refusee_avant_toute_allocation() {
    let flux = serveur(&script_sans_auth(65535, 65535)).await;
    let issue = connecteur(flux).try_start().await;
    let Err(e) = issue else {
        panic!("un cadre de 65535x65535 a été accepté")
    };
    assert!(e.to_string().contains("inacceptable"), "{e}");
}

/// Les décodeurs allouent à la taille du rectangle avant de lire : un
/// rectangle qui déborde du cadre est refusé, et un rectangle qui y tient
/// arrive comme image.
#[tokio::test]
async fn un_rectangle_hors_du_cadre_est_refuse_et_un_rectangle_dedans_passe() {
    let mut s = script_sans_auth(4, 4);
    // Une mise à jour d'un rectangle brut 2×2 en (0,0) : accepté.
    s.extend_from_slice(&[0, 0, 0, 1]);
    s.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0]);
    s.extend_from_slice(&[7; 16]);
    // Puis un rectangle 4×4 en (2,2), qui déborde : refusé, sans lire ses
    // 64 octets (qui ne sont d'ailleurs pas là).
    s.extend_from_slice(&[0, 0, 0, 1]);
    s.extend_from_slice(&[0, 2, 0, 2, 0, 4, 0, 4, 0, 0, 0, 0]);
    let flux = serveur(&s).await;
    let client = connecteur(flux)
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let mut evenements = client.take_events().await.expect("file des événements");
    assert!(
        client.take_events().await.is_none(),
        "la file ne se prend qu'une fois"
    );
    let mut image_vue = false;
    loop {
        match evenements.recv().await {
            Some(VncEvent::SetResolution(_)) => {}
            Some(VncEvent::RawImage(rect, data)) => {
                assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 0, 2, 2));
                assert_eq!(data, vec![7; 16]);
                image_vue = true;
            }
            Some(VncEvent::Error(message)) => {
                assert!(message.contains("hors du cadre"), "{message}");
                break;
            }
            autre => panic!("événement inattendu : {autre:?}"),
        }
    }
    assert!(image_vue, "le rectangle dans le cadre n'est jamais arrivé");
}

/// Après `take_events`, le client refuse de lire lui-même : les deux voies ne
/// coexistent pas.
#[tokio::test]
async fn apres_take_events_le_client_ne_lit_plus_lui_meme() {
    let flux = serveur(&script_sans_auth(4, 4)).await;
    let client = connecteur(flux)
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let _file = client.take_events().await.expect("file des événements");
    assert!(client.poll_event().await.is_err());
    assert!(client.recv_event().await.is_err());
}
