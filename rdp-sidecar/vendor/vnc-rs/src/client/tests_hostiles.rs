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

/// Comme `connecteur`, mais le serveur est déjà connu sous TLS : une ligne
/// `cle` (`vnc:<hôte>:<port>`) existe, donc TLS est exigé (modèle HSTS).
fn connecteur_exigeant_tls(
    flux: DuplexStream,
    cle: &str,
) -> crate::client::connector::VncState<
    DuplexStream,
    impl std::future::Future<Output = Result<String, crate::VncError>> + Send + Sync + 'static,
> {
    VncConnector::new(flux)
        .set_auth_method(async { Ok("secret".to_owned()) })
        .exiger_tls(Some(cle.to_owned()))
        .add_encoding(VncEncoding::Raw)
        .set_pixel_format(PixelFormat::rgba())
        .build()
        .unwrap()
}

/// Un serveur qui écrit `script`, puis lit tout ce que le client lui envoie
/// jusqu'à la fermeture du flux, rendu par le `JoinHandle`. Sert à prouver ce
/// que le client a — ou n'a pas — envoyé avant de renoncer.
fn serveur_qui_ecoute(script: Vec<u8>) -> (DuplexStream, tokio::task::JoinHandle<Vec<u8>>) {
    let (client, mut serveur) = duplex(1 << 16);
    let handle = tokio::spawn(async move {
        serveur.write_all(&script).await.unwrap();
        let mut recu = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut serveur, &mut recu).await;
        recu
    });
    (client, handle)
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

/// Le protocole VNC veut que le client saute les types de sécurité qu'il ne
/// connaît pas et en choisisse un qu'il parle. Un serveur macOS (ARD 30, 33,
/// 35, 36) ou UltraVNC/RealVNC qui annonce de tels types À CÔTÉ de VncAuth
/// faisait au contraire tomber toute la poignée de main (« Unknow VNC security
/// type: 30 »). La connexion doit aboutir sur VncAuth. Trouvé par l'audit du
/// 7 septembre 2026.
#[tokio::test]
async fn un_type_de_securite_inconnu_a_cote_de_vnc_auth_ne_bloque_pas() {
    let mut s = VERSION.to_vec();
    // Trois types : 30 et 33 hors énumération (ARD macOS), 2 = VncAuth.
    s.extend_from_slice(&[3, 30, 33, 2]);
    s.extend_from_slice(&[0x5a; 16]); // défi VncAuth
    s.extend_from_slice(&0u32.to_be_bytes()); // AuthResult : ok
    s.extend_from_slice(&4u16.to_be_bytes()); // largeur
    s.extend_from_slice(&4u16.to_be_bytes()); // hauteur
    s.extend_from_slice(&FORMAT);
    s.extend_from_slice(&1u32.to_be_bytes());
    s.push(b't');
    let flux = serveur(&s).await;
    let issue = connecteur(flux).try_start().await;
    assert!(
        issue.is_ok(),
        "un type inconnu à côté de VncAuth doit laisser la connexion aboutir : {:?}",
        issue.err()
    );
}

/// Une liste qui ne contient QUE des types inconnus doit rendre une erreur
/// explicite nommant les types reçus, jamais une panique ni l'opaque « Unknow
/// VNC security type ». Trouvé par l'audit du 7 septembre 2026.
#[tokio::test]
async fn une_liste_de_types_tous_inconnus_donne_une_erreur_explicite() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[2, 30, 33]); // deux types, tous deux hors énumération
    let flux = serveur(&s).await;
    let Err(e) = connecteur(flux).try_start().await else {
        panic!("une liste de types tous inconnus a été acceptée")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("inconnus") && msg.contains("30") && msg.contains("33"),
        "l'erreur doit nommer les types inconnus reçus : {msg}"
    );
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
            Some(VncEvent::SetResolution(_) | VncEvent::UpdateDone) => {}
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

/// Une mise à jour sans le moindre pixel doit tout de même signaler sa fin,
/// sinon le client — qui ne redemande qu'après une image — attendrait à jamais
/// une image que le serveur attend, lui, qu'on redemande : bureau figé.
///
/// Trouvé par l'audit du 7 septembre 2026 : à un changement de résolution,
/// TigerVNC/x11vnc/QEMU envoient une `FramebufferUpdate` d'un seul
/// pseudo-rectangle DesktopSize (aucun pixel), et certains serveurs une mise à
/// jour à zéro rectangle. Ni l'une ni l'autre ne produisait de signal de fin,
/// et le `test-vnc-server` ne change jamais de résolution, d'où l'angle mort.
#[tokio::test]
async fn chaque_mise_a_jour_signale_sa_fin_meme_sans_pixel() {
    let mut s = script_sans_auth(4, 4);
    // Mise à jour 1 : un seul pseudo-rectangle DesktopSize 8×8 (encodage -223),
    // sans un octet de pixel — ce que le serveur envoie au changement de mode.
    s.extend_from_slice(&[0, 0, 0, 1]); // type 0, padding, 1 rectangle
    s.extend_from_slice(&[0, 0, 0, 0, 0, 8, 0, 8, 0xFF, 0xFF, 0xFF, 0x21]);
    // Mise à jour 2 : zéro rectangle.
    s.extend_from_slice(&[0, 0, 0, 0]); // type 0, padding, 0 rectangle
    let flux = serveur(&s).await;
    let client = connecteur(flux)
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let mut evenements = client.take_events().await.expect("file des événements");
    let mut fins = 0u32;
    let mut redimension_vu = false;
    // Chacune des deux mises à jour doit produire un `UpdateDone` ; sans lui, le
    // `recv` resterait bloqué (le serveur ne parle plus, le client n'ose plus
    // redemander) et le délai ci-dessous ferait tomber le test.
    while fins < 2 {
        match tokio::time::timeout(std::time::Duration::from_secs(3), evenements.recv()).await {
            Ok(Some(VncEvent::UpdateDone)) => fins += 1,
            Ok(Some(VncEvent::SetResolution(s))) => {
                if s.width == 8 && s.height == 8 {
                    redimension_vu = true;
                }
            }
            Ok(Some(VncEvent::Error(m))) => panic!("erreur inattendue : {m}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("le serveur a fermé avant les deux fins de mise à jour"),
            Err(_) => panic!(
                "aucun signal de fin de mise à jour : le bureau resterait figé après un \
                 changement de résolution"
            ),
        }
    }
    assert!(
        redimension_vu,
        "le pseudo-rectangle DesktopSize n'a pas été rapporté comme redimensionnement"
    );
}

/// Un serveur déjà connu sous TLS (une ligne `vnc:<hôte>:<port>` existe, donc
/// l'appelant a posé `exiger_tls`) ne doit pas pouvoir retomber en RFB clair.
/// Le script ci-dessous est celui d'une authentification VNC classique QUI
/// RÉUSSIT : sans la garde, le client choisit VncAuth, répond au défi DES et se
/// connecte en clair. La garde doit refuser AVANT tout envoi du choix de
/// sécurité et de la réponse DES — le serveur ne doit avoir reçu que les douze
/// octets de la version.
///
/// Trouvé par l'audit du 7 septembre 2026 : un interposeur n'a pas à casser
/// TLS ; il lui suffit de réécrire, en clair, la liste des types de sécurité
/// pour en retirer VeNCrypt (type 19). Le client livrait alors sa réponse DES
/// et toute la session en clair à qui se substituait au serveur, sans jamais
/// consulter l'empreinte déjà épinglée.
#[tokio::test]
async fn le_serveur_deja_connu_en_tls_refuse_la_retrogradation() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 2]); // un seul type : VncAuth (pas de 19)
    s.extend_from_slice(&[0x5a; 16]); // défi VncAuth
    s.extend_from_slice(&0u32.to_be_bytes()); // AuthResult : ok
    s.extend_from_slice(&4u16.to_be_bytes()); // largeur
    s.extend_from_slice(&4u16.to_be_bytes()); // hauteur
    s.extend_from_slice(&FORMAT);
    s.extend_from_slice(&1u32.to_be_bytes());
    s.push(b't');
    let (flux, ecoute) = serveur_qui_ecoute(s);
    let issue = connecteur_exigeant_tls(flux, "vnc:S:5900")
        .try_start()
        .await;
    let Err(e) = issue else {
        panic!("un serveur connu sous TLS a été accepté en RFB clair")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("TLS") && msg.contains("vnc:S:5900") && msg.contains("rdp_known_hosts"),
        "l'erreur doit nommer l'exigence de TLS et la ligne à retirer : {msg}"
    );
    // La preuve que rien n'a fui : le client n'a envoyé que sa version (12 o),
    // ni l'octet de choix VncAuth, ni la réponse DES de 16 octets.
    let recu = ecoute.await.unwrap();
    assert_eq!(
        recu.len(),
        VERSION.len(),
        "le client a envoyé plus que sa version : le mot de passe a pu fuir ({recu:?})"
    );
}
