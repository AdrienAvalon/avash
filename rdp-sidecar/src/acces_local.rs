//! Canal local vers l'interface : jeton d'une seule vie, contrôle d'origine du WebSocket.

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// Extrait la valeur d'un jeton de routage.
///
/// Le serveur envoie le jeton complet — `Cookie: msts=2464288595\r\n` — tandis
/// que la bibliothèque ajoute elle-même le préfixe et le terminateur. Le passer
/// tel quel produisait `Cookie: msts=Cookie: msts=…`, que le serveur refusait
/// en fermant la connexion sans un mot.
pub(crate) fn valeur_du_jeton(brut: &[u8]) -> String {
    String::from_utf8_lossy(brut)
        .trim_end_matches(['\r', '\n'])
        .trim_start_matches("Cookie: msts=")
        .to_owned()
}

/// Ce qu'une session a donné.
/// Le poste de travail côté interface : l'écoute locale et le client accepté.
///
/// Il survit aux reconnexions RDP. Une redirection de serveur rétablit la
/// session distante par en dessous ; l'interface, elle, garde le même port, le
/// même jeton et la même WebSocket, et n'a rien à réapprendre.
pub(crate) struct Poste {
    pub(crate) _listener: TcpListener,
    pub(crate) sink:
        futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>,
    pub(crate) stream:
        futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<TcpStream>>,
}

/// Le couple (émetteur, récepteur) d'un WebSocket accepté, transmis d'une tâche
/// de validation vers la boucle d'acceptation.
pub(crate) type PosteSplit = (
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>,
    futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<TcpStream>>,
);

/// Compare deux jetons en temps constant : la durée ne dépend pas de la position
/// du premier octet qui diffère. Le `==` de tranches s'arrête au premier écart,
/// ce qui, en théorie, laisse deviner le jeton octet par octet. Non exploitable
/// ici (jeton de 16 octets, comparaison noyée dans la gigue d'une boucle TCP en
/// loopback), mais gratuit à faire correctement.
pub(crate) fn jetons_egaux(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Rappel de validation d'origine pour `accept_hdr_async`. Fonction nommée (et
/// non closure) pour porter l'`allow` : le type d'erreur imposé par tungstenite
/// est volumineux, mais on ne le construit qu'au rejet d'un client — jamais sur
/// le chemin normal.
#[allow(clippy::result_large_err)]
pub(crate) fn verifier_origine(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    resp: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    let origine = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if origine_admise(origine) {
        Ok(resp)
    } else {
        let mut refus = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
            Some("origine non autorisée".to_owned()),
        );
        *refus.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
        Err(refus)
    }
}

/// Décide si une origine WebSocket est admise. Une page web réelle porte
/// `http(s)://<domaine>` : on la refuse. La webview native porte `tauri://…`
/// (Linux/macOS) ou `http(s)://tauri.localhost` (Windows) ; le serveur de
/// développement, `http://localhost:<port>`. Une absence d'origine est admise —
/// certains clients n'en posent pas, et le jeton reste l'authentification réelle.
///
/// Le tri se fait sur une copie en minuscules (un navigateur normalise le schéma,
/// mais on ne s'y fie pas) et refuse par défaut : seuls les schémas explicitement
/// attendus (tauri://) passent, tout autre (`file://`, `null`, `data:`…) est
/// rejeté. Fail-closed — le laxisme précédent n'était que de la défense en
/// profondeur, autant qu'elle ferme réellement.
fn origine_admise(origine: Option<&str>) -> bool {
    let Some(o) = origine else {
        return true;
    };
    let o = o.to_ascii_lowercase();
    if let Some(reste) = o
        .strip_prefix("http://")
        .or_else(|| o.strip_prefix("https://"))
    {
        let hote = reste.split(['/', ':']).next().unwrap_or(reste);
        hote == "tauri.localhost" || hote == "localhost" || hote == "127.0.0.1"
    } else {
        // Seule la webview native (schéma tauri://) est admise hors http(s) ; tout
        // autre schéma est refusé plutôt qu'admis par défaut.
        o.starts_with("tauri://")
    }
}

#[cfg(test)]
mod tests_acces_local {
    use super::{jetons_egaux, origine_admise};

    #[test]
    fn jetons_egaux_ne_depend_pas_de_la_position_du_premier_ecart() {
        assert!(jetons_egaux(b"0123456789abcdef", b"0123456789abcdef"));
        assert!(!jetons_egaux(b"0123456789abcdef", b"0123456789abcdeg"));
        assert!(!jetons_egaux(b"x123456789abcdef", b"0123456789abcdef"));
        // Longueurs différentes : refus sans lire plus loin.
        assert!(!jetons_egaux(b"court", b"beaucoup plus long"));
        assert!(!jetons_egaux(b"", b"x"));
        assert!(jetons_egaux(b"", b""));
    }

    #[test]
    fn une_page_web_reelle_est_refusee() {
        assert!(!origine_admise(Some("http://evil.example")));
        assert!(!origine_admise(Some("https://evil.example:8443")));
        assert!(!origine_admise(Some("https://cdn.attaquant.net/x")));
    }

    #[test]
    fn la_webview_native_et_le_developpement_passent() {
        assert!(origine_admise(None)); // pas d'en-tête : le jeton fait foi
        assert!(origine_admise(Some("tauri://localhost")));
        assert!(origine_admise(Some("http://tauri.localhost")));
        assert!(origine_admise(Some("https://tauri.localhost")));
        assert!(origine_admise(Some("http://localhost:1420"))); // vite dev
        assert!(origine_admise(Some("http://127.0.0.1:5173")));
    }

    #[test]
    fn un_sous_domaine_de_tauri_localhost_ne_passe_pas() {
        // « tauri.localhost.evil.com » ne doit pas être pris pour tauri.localhost.
        assert!(!origine_admise(Some("http://tauri.localhost.evil.com")));
    }

    #[test]
    fn la_casse_du_schema_ne_contourne_pas_le_controle() {
        // Un schéma en majuscules ne doit pas basculer dans la branche « admis ».
        assert!(!origine_admise(Some("HTTP://evil.example")));
        assert!(!origine_admise(Some("HtTpS://evil.example")));
        assert!(origine_admise(Some("HTTP://localhost:1420")));
    }

    #[test]
    fn un_schema_inattendu_est_refuse_par_defaut() {
        // Fail-closed : file://, data:, null… ne sont pas admis.
        assert!(!origine_admise(Some("file:///etc/passwd")));
        assert!(!origine_admise(Some("null")));
        assert!(!origine_admise(Some("data:text/html,x")));
        // La webview native reste admise.
        assert!(origine_admise(Some("TAURI://localhost")));
    }
}

#[cfg(test)]
mod tests_jeton {
    use super::valeur_du_jeton;

    #[test]
    fn le_prefixe_et_le_terminateur_sont_retires() {
        // Ce que GNOME Remote Desktop envoie réellement.
        assert_eq!(
            valeur_du_jeton(b"Cookie: msts=2464288595\r\n"),
            "2464288595"
        );
    }

    #[test]
    fn une_valeur_deja_nue_passe_telle_quelle() {
        assert_eq!(valeur_du_jeton(b"2464288595"), "2464288595");
    }

    #[test]
    fn un_jeton_vide_ne_panique_pas() {
        assert_eq!(valeur_du_jeton(b""), "");
        assert_eq!(valeur_du_jeton(b"Cookie: msts=\r\n"), "");
    }
}
