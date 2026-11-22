# Ferroterm Android

Client SSH, SFTP et FTP/FTPS natif pour Android 15 et plus (Kotlin, Jetpack Compose).
Indépendant du code Rust du bureau ; le format JSON des sessions est commun (`protocol`,
`ftp`, `key_path`, `port`…), comme le fichier `known_certificates`.

## Briques

| Rôle | Bibliothèque |
|---|---|
| SSH, SFTP | [sshj](https://github.com/hierynomus/sshj) + BouncyCastle complet (remplace celui d'Android au démarrage) |
| FTP, FTPS | [Apache Commons Net](https://commons.apache.org/proper/commons-net/) |
| Terminal | [termlib](https://github.com/connectbot/termlib) de ConnectBot (libvterm, Apache 2.0) |
| Mots de passe | Android Keystore, AES-GCM (`SecretStore`) |
| Connexions en arrière-plan | service de premier plan `specialUse` (`ConnectionService`) |

## Types de session

Comme sur le bureau :

- **SSH** : terminal et explorateur de fichiers (SFTP) sur la même connexion ;
- **SFTP** : explorateur seul, par SSH ;
- **FTP** : explorateur seul ; chiffrement « TLS si disponible » (repli en clair si le
  serveur refuse `AUTH TLS`, avec avertissement), TLS explicite obligatoire, TLS
  implicite ou aucun ; mode passif (adresse de la connexion de contrôle) ou actif.

## Fonctionnement

- Clés serveur SSH dans `files/known_hosts` (format OpenSSH) : hôte inconnu → empreinte
  SHA-256 montrée à l'utilisateur ; clé changée → connexion refusée, remplacement
  possible après confirmation.
- Certificats FTPS : vérifiés avec les autorités du téléphone ; un certificat non reconnu
  (auto-signé, expiré, autre nom) est soumis avec son empreinte puis retenu dans
  `files/known_certificates`.
- FTPS et reprise de session TLS : les connexions de données reprennent la session de
  la connexion de contrôle (exigé par vsftpd `require_ssl_reuse`, FileZilla Server…).
  Le cache TLS d'Android étant indexé par hôte et port, les sockets de données sont
  ouvertes sous le nom de la connexion de contrôle (`ftp/Certificates.kt`).
- Authentification : mot de passe (puis clavier-interactif en SSH), ou clé privée
  importée (OpenSSH, PEM, PuTTY) avec phrase de passe demandée et mémorisable. FTP :
  `anonymous` pour une connexion anonyme.
- Explorateur (`files/FileBrowser.kt`, commun SFTP/FTP) : navigation, nouveau dossier,
  renommage, suppression (dossiers vides seulement, comme sur le bureau), envoi et
  téléchargement avec progression et annulation, ouverture dans une autre application.
  En FTP, chaque transfert a sa propre connexion (deux au plus) : la navigation reste
  possible pendant les transferts.
- Supervision (sessions SSH, serveurs Linux) : même script que le bureau (`resources/com/ferroterm/monitor/monitor.sh`), sur un canal dédié de la connexion du terminal, relancé s'il se ferme. Résumé CPU / RAM sous le titre du terminal (le toucher ouvre l'écran « Serveur » : système, courbe CPU, mémoire, réseau, disques, processus). L'explorateur peut aller au dossier courant du terminal.
- Macros (`files/macros.json`, mêmes champs que le bureau) : depuis le terminal, exécutées ou posées sur la ligne de commande sans Entrée.
- Rangée de touches au-dessus du clavier : Échap, Tab, Ctrl/Alt collants (un appui :
  touche suivante ; deux : verrouillés), flèches, Orig/Fin, Pg↑/Pg↓.

## Compiler

```sh
./gradlew assembleDebug        # APK : app/build/outputs/apk/debug/
./gradlew testDebugUnitTest    # tests unitaires
```

`local.properties` doit indiquer le SDK (`sdk.dir=…`), ce que fait Android Studio.

## Pas encore porté

Édition de fichiers distants dans l'application, scission d'écran (plusieurs terminaux côte à côte).
