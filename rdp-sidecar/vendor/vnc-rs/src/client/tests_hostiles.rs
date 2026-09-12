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
/// le client doit avoir pu écrire ses propres messages avant que le tuyau ne
/// casse, et une fin de flux au milieu d'une raison d'échec reste un refus.
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

/// Monteur TLS « identité » : sur un `DuplexStream` (tampon en mémoire),
/// « passer sous TLS » ne change pas le flux, ce qui suffit à scénariser
/// `vencrypt()` sans certificat ni pair TLS. Le vrai monteur (vnc.rs) échange le
/// flux clair contre un flux chiffré ; ici le type `S` reste le même, donc
/// l'identité convient. Trouvé par l'audit du 7 septembre 2026 : la voie
/// VeNCrypt du client n'était éprouvée que par la suite bout en bout, jamais en
/// test unitaire.
fn upgrader_identite() -> crate::client::connector::TlsUpgrader<DuplexStream> {
    Box::new(|flux| {
        Box::pin(async move { Ok(flux) })
            as std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<DuplexStream, crate::VncError>>
                        + Send
                        + Sync,
                >,
            >
    })
}

/// Comme `connecteur`, mais l'appelant sait monter TLS : le `tls_upgrader` est
/// posé, donc le client choisit VeNCrypt (type 19) quand le serveur l'offre
/// (comme vnc.rs le pose en production, inconditionnellement).
fn connecteur_avec_tls(
    flux: DuplexStream,
) -> crate::client::connector::VncState<
    DuplexStream,
    impl std::future::Future<Output = Result<String, crate::VncError>> + Send + Sync + 'static,
> {
    VncConnector::new(flux)
        .set_auth_method(async { Ok("secret".to_owned()) })
        .set_tls_upgrader(upgrader_identite())
        .add_encoding(VncEncoding::Raw)
        .set_pixel_format(PixelFormat::rgba())
        .build()
        .unwrap()
}

/// Le `ServerInit` d'un bureau 4×4 sans nom particulier, ce que `VncClient::new`
/// lit une fois l'authentification (VeNCrypt ou VNC) passée.
fn server_init_4x4() -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&4u16.to_be_bytes()); // largeur
    s.extend_from_slice(&4u16.to_be_bytes()); // hauteur
    s.extend_from_slice(&FORMAT);
    s.extend_from_slice(&1u32.to_be_bytes()); // longueur du nom
    s.push(b't');
    s
}

/// Comme `serveur_qui_ecoute`, mais le serveur ne lit QUE `a_lire` octets du
/// client puis ferme le flux : tout `read` du client resté en attente reçoit
/// alors une fin de flux et le client renonce, au lieu d'attendre à jamais une
/// réponse que ce serveur figé ne donnera pas. Sert à prouver le choix du
/// client (les premiers octets) sans risque d'interblocage, quel que soit le
/// chemin qu'il prend.
fn serveur_qui_ecoute_borne(
    script: Vec<u8>,
    a_lire: usize,
) -> (DuplexStream, tokio::task::JoinHandle<Vec<u8>>) {
    let (client, mut serveur) = duplex(1 << 16);
    let handle = tokio::spawn(async move {
        serveur.write_all(&script).await.unwrap();
        let mut recu = vec![0u8; a_lire];
        tokio::io::AsyncReadExt::read_exact(&mut serveur, &mut recu)
            .await
            .unwrap();
        drop(serveur);
        recu
    });
    (client, handle)
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
    // raison vaut les quatre octets annoncés, la fermeture n'y change rien.
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
    // raccroche, après une raison qui vaut les trois octets annoncés.
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

/// La longueur du nom du bureau (ServerInit, name-length en u32) était allouée
/// telle quelle (`vec![0; name_len]`), sans passer par la borne du codec : un
/// serveur hostile annonçant 0xFFFFFFFF faisait réclamer 4 Gio avant de lire un
/// octet — abandon du processus sous Windows (`handle_alloc_error`), attente
/// jusqu'au délai de lecture sous Linux (l'allocation à zéro y est paresseuse,
/// d'où l'angle mort de la cible fuzz). Le nom doit être borné AVANT toute
/// allocation, comme le texte du presse-papiers. Trouvé par l'audit du
/// 7 septembre 2026.
#[tokio::test]
async fn un_nom_de_bureau_deraisonnable_est_refuse_avant_toute_allocation() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 1]); // un seul type de sécurité : None
    s.extend_from_slice(&0u32.to_be_bytes()); // SecurityResult : ok
    s.extend_from_slice(&4u16.to_be_bytes()); // largeur
    s.extend_from_slice(&4u16.to_be_bytes()); // hauteur
    s.extend_from_slice(&FORMAT);
    s.extend_from_slice(&u32::MAX.to_be_bytes()); // name-length = 0xFFFFFFFF
                                                  // ...et pas un octet de nom : la borne doit tomber avant toute lecture. Le
                                                  // serveur lit les 14 octets que le client envoie (version 12, choix None 1,
                                                  // drapeau partagé 1) puis raccroche, pour qu'un échec de lecture (ancien
                                                  // code, après une allocation de 4 Gio) se distingue de la borne du codec
                                                  // (code corrigé, aucune allocation).
    let flux = serveur_qui_raccroche(s, VERSION.len() + 1 + 1);
    let Err(e) = connecteur(flux).try_start().await else {
        panic!("un nom de bureau de 0xFFFFFFFF octets a été accepté")
    };
    assert!(
        e.to_string().contains("borne"),
        "le nom doit être refusé par la borne du codec avant toute allocation : {e}"
    );
}

/// Un serveur hostile peut coder une tuile TRLE à palette de 2 couleurs puis y
/// désigner un indice hors palette : l'octet de contrôle du RLE indexé porte
/// `index = control & 0x7f`, jusqu'à 127. `copy_indexed` tranchait alors
/// `palette[start..start + bpp]` hors des bornes et paniquait la tâche de
/// décodage tokio. La panique n'abattait pas tout le sidecar (pas de
/// `panic=abort`), mais elle fermait le canal de sortie : `recv` rendait `None`
/// et l'utilisateur voyait « Le serveur a fermé la connexion. » (avec la ligne
/// de panique en incrustation) alors que c'est le client qui avait planté. Un
/// indice hors palette doit devenir une `VncEvent::Error` franche.
///
/// Trouvé par l'audit du 7 septembre 2026 : la cible fuzz ne construisait jamais
/// de flux TRLE/ZRLE valide (elle ne vérifiait qu'un invariant de taille), d'où
/// l'angle mort. La même faille existe en ZRLE (codage que le client demande),
/// mais elle exigerait un flux zlib forgé ; le chemin `copy_indexed` est
/// identique et éprouvé ici en TRLE, non compressé.
#[tokio::test]
async fn un_indice_de_palette_hors_borne_en_trle_est_une_erreur_pas_une_panique() {
    let mut s = script_sans_auth(4, 4);
    // Une mise à jour d'un seul rectangle 4×4 en (0,0), codé TRLE (encodage 15).
    s.extend_from_slice(&[0, 0, 0, 1]);
    s.extend_from_slice(&[0, 0, 0, 0, 0, 4, 0, 4, 0, 0, 0, 15]);
    // Ce portage lit d'abord une longueur u32 (bloc préfixé, ignoré ici avec 0)
    // puis la tuile depuis le flux. Tuile TRLE : octet de contrôle 0x82 (RLE
    // indexé, palette de 2 couleurs), six octets CPIXEL (deux couleurs sur
    // 3 octets chacune), puis 0x05 : indice 5, série de 1. La palette n'a que
    // deux entrées (indices 0 et 1) : l'indice 5 vise palette[20..24] sur
    // 8 octets.
    s.extend_from_slice(&[0, 0, 0, 0]);
    s.extend_from_slice(&[0x82, 1, 2, 3, 4, 5, 6, 0x05]);
    let flux = serveur(&s).await;
    let client = connecteur(flux)
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let mut evenements = client.take_events().await.expect("file des événements");
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(3), evenements.recv()).await {
            Ok(Some(VncEvent::Error(message))) => {
                // Chemin corrigé : une erreur d'image franche, pas une panique.
                assert!(
                    message.contains("decoded"),
                    "l'erreur doit venir d'une donnée d'image invalide : {message}"
                );
                return;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!(
                "le canal de sortie s'est fermé sans erreur : la tâche de décodage a paniqué \
                 sur l'indice de palette hors borne"
            ),
            Err(_) => panic!("aucun événement : le décodeur n'a ni abouti ni signalé d'erreur"),
        }
    }
}

/// VeNCrypt (type 19) doit être préféré à l'authentification VNC en clair
/// (type 2) quand le serveur offre les deux ET que l'appelant sait monter TLS —
/// ce qu'attendent les serveurs réels (TigerVNC avec
/// `SecurityTypes=VeNCrypt,X509Vnc,VncAuth`). Le seul scénario TLS bout en bout
/// n'offrait qu'un type (19) : il prouvait que le client SAIT faire VeNCrypt,
/// jamais qu'il le PRÉFÈRE au clair. Une inversion de la condition de
/// préférence dans `Authenticate` (tester `VncAuth` avant VeNCrypt) ferait
/// basculer les serveurs réels en clair sans rien faire rougir.
///
/// Trouvé par l'audit du 7 septembre 2026 : `vencrypt()` n'avait aucun test
/// unitaire, et le terminateur du serveur de test n'annonçait que le type 19.
/// La preuve est directe : le premier octet écrit après la version est le type
/// de sécurité choisi ; il doit valoir 19, pas 2.
#[tokio::test]
async fn vencrypt_est_prefere_a_l_auth_vnc_quand_les_deux_sont_offerts() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[2, 2, 19]); // deux types offerts : VncAuth (2) ET VeNCrypt (19)
    s.extend_from_slice(&[0, 2]); // version VeNCrypt du serveur : 0.2
    s.push(1); // ack de version ≠ 0 : le client renonce ici, après avoir choisi
               // Le serveur lit la version (12 o) et l'octet de choix, puis ferme : que le
               // client ait pris VeNCrypt (il bloquerait ensuite sur l'ack) ou VncAuth (il
               // bloquerait sur le défi), il est débloqué et renonce — pas d'interblocage.
    let (flux, ecoute) = serveur_qui_ecoute_borne(s, VERSION.len() + 1);
    let issue = connecteur_avec_tls(flux).try_start().await;
    assert!(
        issue.is_err(),
        "le serveur a refusé la version VeNCrypt : la connexion ne doit pas aboutir"
    );
    let recu = ecoute.await.unwrap();
    assert_eq!(
        recu[VERSION.len()],
        19,
        "le client a choisi le type {} au lieu de VeNCrypt (19) : il retombe en clair alors \
         que VeNCrypt était offert",
        recu[VERSION.len()]
    );
}

/// VeNCrypt n'accepte que les sous-types X.509 : les sous-types TLS anonymes
/// (Diffie-Hellman sans certificat, 256/257) ne prouvent pas à qui l'on parle.
/// Un serveur qui n'offre qu'eux doit être refusé par une erreur nommant X.509,
/// jamais accepté en silence. Trouvé par l'audit du 7 septembre 2026.
#[tokio::test]
async fn vencrypt_refuse_les_sous_types_tls_anonymes() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 19]); // un seul type : VeNCrypt
    s.extend_from_slice(&[0, 2]); // version VeNCrypt 0.2
    s.push(0); // ack de version : ok
    s.push(2); // deux sous-types
    s.extend_from_slice(&256u32.to_be_bytes()); // TLSNone (anonyme)
    s.extend_from_slice(&257u32.to_be_bytes()); // TLSVnc (anonyme)
    let flux = serveur(&s).await;
    let Err(e) = connecteur_avec_tls(flux).try_start().await else {
        panic!("un serveur VeNCrypt sans sous-type X.509 a été accepté")
    };
    assert!(
        e.to_string().contains("X.509"),
        "l'erreur doit nommer X.509 et le refus des sous-types anonymes : {e}"
    );
}

/// Sous-type X509Vnc (261) : après le passage TLS, le client répond au défi
/// VNC (DES) comme en authentification VNC classique, puis se connecte. Trouvé
/// par l'audit du 7 septembre 2026 : cette voie n'avait aucun test unitaire.
#[tokio::test]
async fn vencrypt_x509vnc_repond_au_defi_et_se_connecte() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 19]); // VeNCrypt
    s.extend_from_slice(&[0, 2]); // version 0.2
    s.push(0); // ack de version
    s.push(1); // un sous-type
    s.extend_from_slice(&261u32.to_be_bytes()); // X509Vnc
    s.push(1); // ack du sous-type choisi
               // TLS (identité), puis défi VNC de 16 octets et résultat.
    s.extend_from_slice(&[0x5a; 16]); // défi
    s.extend_from_slice(&0u32.to_be_bytes()); // AuthResult : ok
    s.extend_from_slice(&server_init_4x4());
    let flux = serveur(&s).await;
    connecteur_avec_tls(flux)
        .try_start()
        .await
        .expect("VeNCrypt X509Vnc doit aboutir")
        .finish()
        .expect("le client doit être connecté");
}

/// Sous-type X509None (260) : après le passage TLS, le serveur n'envoie qu'un
/// résultat de sécurité (pas de défi), et le client se connecte. Trouvé par
/// l'audit du 7 septembre 2026 : cette voie n'avait aucun test unitaire.
#[tokio::test]
async fn vencrypt_x509none_lit_le_seul_resultat_et_se_connecte() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 19]); // VeNCrypt
    s.extend_from_slice(&[0, 2]); // version 0.2
    s.push(0); // ack de version
    s.push(1); // un sous-type
    s.extend_from_slice(&260u32.to_be_bytes()); // X509None
    s.push(1); // ack du sous-type choisi
               // TLS (identité), puis le seul résultat de sécurité.
    s.extend_from_slice(&0u32.to_be_bytes()); // résultat : ok
    s.extend_from_slice(&server_init_4x4());
    let flux = serveur(&s).await;
    connecteur_avec_tls(flux)
        .try_start()
        .await
        .expect("VeNCrypt X509None doit aboutir")
        .finish()
        .expect("le client doit être connecté");
}

/// Le client ne parle que VeNCrypt 0.2 (la seule version qui porte les
/// sous-types sur quatre octets) : une version antérieure annoncée par le
/// serveur doit être refusée par une erreur nommant la version, jamais
/// poursuivie à l'aveugle. Trouvé par l'audit du 7 septembre 2026.
#[tokio::test]
async fn vencrypt_refuse_une_version_anterieure_a_0_2() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 19]); // VeNCrypt
    s.extend_from_slice(&[0, 1]); // version VeNCrypt 0.1 : trop ancienne
    let flux = serveur(&s).await;
    let Err(e) = connecteur_avec_tls(flux).try_start().await else {
        panic!("une version VeNCrypt 0.1 a été acceptée")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("0.1") && msg.contains("non prise en charge"),
        "l'erreur doit nommer la version VeNCrypt refusée : {msg}"
    );
}

/// Un serveur qui refuse le mot de passe puis NE RACCROCHE PAS : la 3.8
/// (§7.1.2) fait suivre le résultat d'une longueur (u32) et d'autant d'octets
/// de raison, mais le client lisait cette longueur pour la jeter
/// (`read_u32().await.is_ok()`) et ramassait la suite avec `read_to_string`,
/// qui n'a d'autre fin que la fermeture du flux. Le sidecar restait alors en
/// lecture jusqu'au délai de connexion (25 s, posé par `rdp-sidecar/src/vnc.rs`)
/// en empilant en mémoire tout ce que le serveur déversait (plusieurs
/// centaines de mégaoctets sur un lien local) pour un simple mot de passe
/// refusé. La raison annoncée doit être lue pour ce qu'elle annonce, rien de
/// plus, et le refus rendu tout de suite.
///
/// Trouvé par l'audit du 9 septembre 2026 : les tests de cette voie ne
/// s'appuyaient que sur des serveurs qui raccrochent volontairement
/// (`serveur_qui_raccroche`), jamais sur un serveur qui garde la connexion
/// ouverte : l'angle mort exact de `read_to_string`.
#[tokio::test]
async fn une_raison_de_refus_ne_se_lit_pas_au_dela_de_la_longueur_annoncee() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 2]); // un seul type : VncAuth
    s.extend_from_slice(&[0x5a; 16]); // défi
    s.extend_from_slice(&1u32.to_be_bytes()); // AuthResult : refusé
    s.extend_from_slice(&3u32.to_be_bytes()); // reason-length
    s.extend_from_slice(b"bad");
    // ...puis le serveur déverse des octets sans jamais fermer : ils ne font
    // pas partie de la raison et ne doivent pas être lus.
    s.extend_from_slice(&[b'Z'; 8192]);
    let flux = serveur(&s).await;
    let issue = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        connecteur(flux).try_start(),
    )
    .await;
    let Ok(issue) = issue else {
        panic!(
            "le client lit encore la raison d'échec : un serveur qui ne raccroche pas le tient \
                jusqu'au délai de connexion"
        )
    };
    let Err(e) = issue else {
        panic!("un mot de passe refusé a été pris pour un succès")
    };
    assert!(
        matches!(e, crate::VncError::WrongPassword),
        "un refus d'authentification doit se présenter comme tel : {e}"
    );
}

/// Même défaut sur la voie VeNCrypt (authentification VNC tunnelée sous TLS) :
/// le premier contact TOFU accepte un certificat encore inconnu, et le serveur
/// d'en face peut alors refuser le mot de passe puis retenir le sidecar en
/// lecture sans fin. Trouvé par l'audit du 9 septembre 2026.
#[tokio::test]
async fn une_raison_de_refus_vencrypt_ne_se_lit_pas_au_dela_de_la_longueur_annoncee() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 19]); // VeNCrypt
    s.extend_from_slice(&[0, 2]); // version 0.2
    s.push(0); // ack de version
    s.push(1); // un sous-type
    s.extend_from_slice(&261u32.to_be_bytes()); // X509Vnc
    s.push(1); // ack du sous-type choisi
               // TLS (identité), puis défi VNC et refus.
    s.extend_from_slice(&[0x5a; 16]); // défi
    s.extend_from_slice(&1u32.to_be_bytes()); // AuthResult : refusé
    s.extend_from_slice(&5u32.to_be_bytes()); // reason-length
    s.extend_from_slice(b"refus");
    s.extend_from_slice(&[b'Z'; 8192]); // et le flux reste ouvert
    let flux = serveur(&s).await;
    let issue = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        connecteur_avec_tls(flux).try_start(),
    )
    .await;
    let Ok(issue) = issue else {
        panic!("le client lit encore la raison d'échec VeNCrypt jusqu'à la fin du flux")
    };
    let Err(e) = issue else {
        panic!("un mot de passe refusé sous VeNCrypt a été pris pour un succès")
    };
    assert!(
        matches!(e, crate::VncError::WrongPassword),
        "un refus d'authentification doit se présenter comme tel : {e}"
    );
}

/// La raison d'un refus de connexion (aucun type de sécurité annoncé, §7.1.2)
/// se lisait de la même façon, jusqu'à la fin du flux : c'est le même défaut,
/// avant même que le mot de passe soit demandé. Trouvé par l'audit du
/// 9 septembre 2026.
#[tokio::test]
async fn une_raison_de_refus_de_connexion_ne_se_lit_pas_au_dela_de_la_longueur_annoncee() {
    let mut s = VERSION.to_vec();
    s.push(0); // aucun type de sécurité : suivi d'une raison
    s.extend_from_slice(&4u32.to_be_bytes());
    s.extend_from_slice(b"nope");
    s.extend_from_slice(&[b'Z'; 8192]); // et le flux reste ouvert
    let flux = serveur(&s).await;
    let issue = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        connecteur(flux).try_start(),
    )
    .await;
    let Ok(issue) = issue else {
        panic!("le client lit encore la raison du refus de connexion jusqu'à la fin du flux")
    };
    let Err(e) = issue else {
        panic!("un serveur sans type de sécurité a été accepté")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("nope") && !msg.contains('Z'),
        "la raison doit valoir exactement les quatre octets annoncés : {msg}"
    );
}

/// Une longueur de raison démesurée (0xFFFFFFFF) ne doit ni faire allouer, ni
/// faire lire au-delà d'une borne : le client lit ce qu'il accepte de lire et
/// rend le refus. Même principe que le nom de bureau de ServerInit, borné par
/// l'audit du 7 septembre 2026 ; ce chemin d'authentification y avait échappé.
/// Trouvé par l'audit du 9 septembre 2026.
#[tokio::test]
async fn une_raison_de_refus_demesuree_est_bornee() {
    let mut s = VERSION.to_vec();
    s.extend_from_slice(&[1, 2]); // VncAuth
    s.extend_from_slice(&[0x5a; 16]); // défi
    s.extend_from_slice(&1u32.to_be_bytes()); // AuthResult : refusé
    s.extend_from_slice(&u32::MAX.to_be_bytes()); // reason-length : 4 Gio annoncés
    s.extend_from_slice(&[b'Z'; 8192]); // de quoi rassasier la borne, sans fermer
    let flux = serveur(&s).await;
    let issue = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        connecteur(flux).try_start(),
    )
    .await;
    let Ok(issue) = issue else {
        panic!("une raison de 0xFFFFFFFF octets tient encore le client jusqu'au délai")
    };
    let Err(e) = issue else {
        panic!("un mot de passe refusé a été pris pour un succès")
    };
    assert!(
        matches!(e, crate::VncError::WrongPassword),
        "un refus d'authentification doit se présenter comme tel : {e}"
    );
}

/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-5) : le texte
/// `ServerCutText` n'était borné que par le tampon prévu pour les pixels
/// (8192 × 8192 × 4, 256 Mio). Un serveur faisait allouer 200 Mio de texte à
/// tout moment, puis le double une fois converti en UTF-8. Le texte a sa propre
/// borne, 16 Mio, tenue avant toute allocation.
#[tokio::test]
async fn un_texte_du_presse_papiers_demesure_est_refuse_avant_toute_allocation() {
    let mut s = script_sans_auth(4, 4);
    s.extend_from_slice(&[3, 0, 0, 0]); // ServerCutText, bourrage
    s.extend_from_slice(&u32::try_from(crate::codec::TEXTE_MAX + 1).unwrap().to_be_bytes());
    // ...et pas un octet de texte : la borne doit tomber avant toute lecture.
    let flux = serveur(&s).await;
    let client = connecteur(flux)
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let mut evenements = client.take_events().await.expect("file des événements");
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(3), evenements.recv()).await {
            Ok(Some(VncEvent::Error(m))) => {
                assert!(m.contains("borne"), "{m}");
                break;
            }
            Ok(Some(VncEvent::Text(_))) => panic!("un texte de 16 Mio + 1 a été accepté"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("la file s'est fermée sans erreur"),
            Err(_) => panic!("aucune erreur : le client attend les octets d'un texte démesuré"),
        }
    }
}
