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

/// Le poste de travail côté interface : l'écoute locale et le client accepté.
///
/// Il survit aux reconnexions RDP. Une redirection de serveur rétablit la
/// session distante par en dessous ; l'interface, elle, garde le même port, le
/// même jeton et la même WebSocket, et n'a rien à réapprendre.
pub struct Poste {
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

/// Ouvre l'écoute locale, annonce « PORT JETON » sur la sortie standard et
/// attend le premier client qui présente le jeton. Sans effet si le poste
/// existe déjà : une redirection RDP rappelle la session, et rouvrir un port
/// neuf laisserait l'interface parler dans le vide, attachée à l'ancien.
pub(crate) async fn etablir_poste(poste: &mut Option<Poste>) -> Result<()> {
    use anyhow::Context as _;
    use tokio::io::AsyncWriteExt as _;

    if poste.is_some() {
        return Ok(());
    }
    // Serveur WebSocket local : un seul client (Avash), jeton obligatoire.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("écoute WebSocket")?;
    let port = listener.local_addr()?.port();
    let token = format!("{:016x}", rand::random::<u64>());
    // Annonce le point de connexion à Avash.
    let mut out = tokio::io::stdout();
    out.write_all(format!("{port} {token}\n").as_bytes())
        .await?;
    out.flush().await?;

    let (sink, stream) = attendre_client(&listener, &token).await;
    *poste = Some(Poste {
        _listener: listener,
        sink,
        stream,
    });
    Ok(())
}

/// Attend, sur une écoute déjà ouverte, le premier client qui présente le bon
/// jeton **en binaire**, et rend son couple (émetteur, récepteur). Tout premier
/// message qui n'est pas un binaire au bon jeton est rejeté et la file continue.
///
/// On boucle sur les connexions au lieu d'en accepter une seule. Le port est
/// ouvert avant même que l'interface n'en soit avertie : n'importe quel
/// processus local — ou une page web, les WebSocket n'étant pas soumises à la
/// politique d'origine pour *établir* la connexion — pouvait s'y présenter le
/// premier. Un message quelconque faisait quitter le sidecar, détruisant une
/// session RDP déjà authentifiée (TLS + NLA refaits) ; une connexion TCP
/// laissée sans poignée de main WebSocket consommait la seule place d'`accept`
/// et l'interface n'arrivait jamais à se connecter. Le jeton (64 bits) reste
/// hors de portée : c'était un déni de service, pas un détournement. On rejette
/// maintenant l'intrus et on attend le suivant, avec un délai de garde par
/// tentative pour qu'un client muet ne bloque pas la file.
///
/// Extraite d'`etablir_poste` (qui garde l'annonce « PORT JETON ») pour être
/// éprouvée directement : trouvé par l'audit du 7 septembre 2026, le contrôle du
/// jeton n'avait aucun test, si bien qu'élargir le motif à n'importe quel
/// premier message aurait laissé entrer un intrus sans faire rougir la suite.
pub(crate) async fn attendre_client(listener: &TcpListener, token: &str) -> PosteSplit {
    use futures_util::StreamExt as _;

    const DELAI_POIGNEE: std::time::Duration = std::time::Duration::from_secs(10);
    // Chaque validation (poignée WebSocket + premier message) dans SA tâche,
    // et l'acceptation continue en parallèle : un client muet n'immobilise
    // plus la file, ce qui fermait la porte à un déni de service par une page
    // web ou un processus local qui ouvrait des connexions sans rien envoyer.
    // On retient le premier client qui présente le bon jeton, puis on cesse
    // d'accepter (les tâches encore en vol tombent avec le canal).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PosteSplit>(1);
    loop {
        tokio::select! {
            Some(pair) = rx.recv() => break pair,
            accepte = listener.accept() => {
                let Ok((tcp, _)) = accepte else { continue };
                tcp.set_nodelay(true).ok();
                let tx = tx.clone();
                let token = token.to_owned();
                tokio::spawn(async move {
                    // Contrôle d'origine (verifier_origine) : une page web réelle
                    // porte http(s)://<domaine> et se voit refusée ; la webview
                    // (tauri://… ou tauri.localhost, localhost en dev) passe. Le
                    // jeton reste requis.
                    let Ok(Ok(ws)) = tokio::time::timeout(
                        DELAI_POIGNEE,
                        tokio_tungstenite::accept_hdr_async(tcp, verifier_origine),
                    )
                    .await
                    else {
                        return; // poignée absente, trop lente, ou origine refusée
                    };
                    let (sink, mut stream) = ws.split();
                    // Premier message = le jeton, en binaire, comparé à temps
                    // constant. Un texte, une trame de contrôle ou un mauvais
                    // jeton ne passent pas : le client est abandonné et la file
                    // se poursuit.
                    if let Ok(Some(Ok(Message::Binary(t)))) =
                        tokio::time::timeout(DELAI_POIGNEE, stream.next()).await
                    {
                        if jetons_egaux(&t, token.as_bytes()) {
                            let _ = tx.send((sink, stream)).await;
                        }
                    }
                });
            }
        }
    }
}

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
mod tests_attendre_client {
    use super::attendre_client;
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    // WebSocket cliente vers l'écoute locale, sans en-tête Origin (le jeton fait
    // foi). Le type complet est nommé une fois ici pour rester lisible ailleurs.
    type ClientWs = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    async fn se_connecter(port: u16) -> ClientWs {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/"))
            .await
            .expect("poignée WebSocket cliente");
        ws
    }

    // Trouvé par l'audit du 7 septembre 2026 : le contrôle du jeton du canal
    // local (`etablir_poste` / `attendre_client`) n'avait aucun test. Sans lui,
    // élargir le motif du premier message (accepter un texte, ou n'importe quelle
    // trame sans comparer le jeton) serait resté vert en `cargo test` comme en
    // E2E, alors que n'importe quel processus local aurait alors pris la session.
    // Ce scénario verrouille « tout premier message qui n'est pas un binaire au
    // bon jeton est rejeté, et la file continue jusqu'au vrai client ».
    #[tokio::test]
    async fn seul_le_client_au_bon_jeton_binaire_est_retenu() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "0123456789abcdef".to_owned();

        let jeton = token.clone();
        let attente = tokio::spawn(async move { attendre_client(&listener, &jeton).await });

        // Intrus 1 : bon format (binaire) mais mauvais jeton.
        let mut mauvais_jeton = se_connecter(port).await;
        mauvais_jeton
            .send(Message::Binary(b"mauvais_jeton___".to_vec().into()))
            .await
            .unwrap();

        // Intrus 2 : le bon jeton, mais présenté en texte et non en binaire.
        let mut en_texte = se_connecter(port).await;
        en_texte
            .send(Message::Text(token.clone().into()))
            .await
            .unwrap();

        // Intrus 3 : muet — poignée faite, aucun message. Il ne doit pas bloquer
        // la file (il tombera de lui-même sur le délai de garde). On le garde en
        // vie jusqu'à la fin du test pour que la connexion reste vraiment ouverte.
        let _muet = se_connecter(port).await;

        // Laisser le sidecar traiter et rejeter les trois : aucun n'est retenu.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !attente.is_finished(),
            "un intrus (mauvais jeton, jeton en texte, ou muet) a été retenu comme client"
        );

        // Client légitime : le bon jeton, en binaire.
        let mut legitime = se_connecter(port).await;
        legitime
            .send(Message::Binary(token.clone().into_bytes().into()))
            .await
            .unwrap();

        // `attendre_client` rend le couple de CE client, et de lui seul.
        let (mut sink, mut stream) =
            tokio::time::timeout(std::time::Duration::from_secs(5), attente)
                .await
                .expect("attendre_client n'a pas rendu la main au client légitime")
                .unwrap();

        // Une trame échangée dans les deux sens prouve que le couple retenu est
        // bien la connexion vivante du client légitime.
        legitime
            .send(Message::Binary(b"ping".to_vec().into()))
            .await
            .unwrap();
        let recu = stream.next().await.unwrap().unwrap();
        assert_eq!(recu, Message::Binary(b"ping".to_vec().into()));

        sink.send(Message::Binary(b"pong".to_vec().into()))
            .await
            .unwrap();
        let echo = legitime.next().await.unwrap().unwrap();
        assert_eq!(echo, Message::Binary(b"pong".to_vec().into()));
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
