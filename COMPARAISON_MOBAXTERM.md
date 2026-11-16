# Ferroterm face à MobaXterm : ce qu'il reste à reprendre

Comparaison faite le 12 novembre 2026 entre ferroterm 0.1.0 et MobaXterm Personal 26.5, limitée aux fonctionnalités que ferroterm possède déjà.

## Comment cette liste a été établie

- **MobaXterm** : libellés et noms d'options extraits de `MobaXterm_Personal_26.5.exe` avec rea 6.8.0 (moteur Ghidra 12.1.4). C'est une analyse statique : MobaXterm n'a pas été lancé, donc le comportement exact de chaque option reste à vérifier au moment de l'implémenter.
- **Ferroterm** : plan du code source (`src/*.rs`) et recherches ciblées. Une fonctionnalité notée « absente » signifie qu'aucune trace n'en a été trouvée dans le code.

## Comment décider

Cochez la case de chaque point à reprendre. La colonne « Priorité » est une proposition : 1 = à faire en premier, 3 = confort.

## Terminal

Ferroterm aujourd'hui : sélection = copie, clic droit = coller, zoom, collage protégé (bracketed paste), historique fixe de 3000 lignes, défilement avec Maj+Page préc./suiv.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [x] Recherche dans le terminal (fait le 12/11/2026, Ctrl+F) | « Find in terminal » | 1 |
| [ ] Enregistrer la sortie dans un fichier | « Save terminal output » | 2 |
| [ ] Journal automatique des sessions | `LogTerminalActivity`, dossier de journaux imposé | 2 |
| [ ] Avertissement avant de coller plusieurs lignes | « You are about to paste several lines… » | 2 |
| [x] Coloration par mots-clés (fait le 16/11/2026 : erreurs / avertissements / succès en FR et EN, listes modifiables, texte déjà coloré respecté ; réglage global, pas encore par session) | Listes de mots-clés par couleur, préréglage OK/warning/error ; choix du jeu par session | 2 |
| [ ] Ctrl+clic sur les URL | Ouverture depuis le terminal, avec alerte sur les liens HTTP ou raccourcis | 3 |
| [ ] Réglages police et couleurs par session | « Terminal Font settings », « Terminal Color settings » | 3 |
| [ ] Taille d'historique réglable | Aujourd'hui fixée à 3000 lignes dans le code | 3 |
| [ ] Réglage de la touche Retour arrière | `BackspaceSendsBS`, `BackspaceIsDelete` | 3 |
| [ ] Imprimer la sortie | « Print terminal output » | 3 |

## Onglets et scissions

Ferroterm aujourd'hui : groupes d'onglets disposés en rangées (chacune avec ses colonnes), glisser-déposer entre groupes, largeurs mémorisées, restauration des terminaux au démarrage.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [ ] Mode multi-exécution | Saisie envoyée à tous les terminaux, avec exclusion d'un onglet et défilement synchronisé | 1 |
| [x] Scission horizontale et grille de 4 (fait le 12/11/2026 ; rangées libres le 14/11/2026 : « Scinder en bas » ajoute une rangée, « Scinder à droite » ajoute un groupe dans la rangée active) | « 2-terminals mode (Horizontal split) », « 4-terminals mode » | 2 |
| [ ] Confirmation avant de fermer une session active | « One or more sessions are currently running… » | 2 |
| [x] Renommer un onglet (fait le 14/11/2026 : double-clic ou clic droit > Renommer, nom conservé au redémarrage) | « Rename tab » | 3 |
| [ ] Détacher un onglet dans sa propre fenêtre | « Detach/Reattach tab » | 3 |

## Explorateur SFTP

Ferroterm aujourd'hui : liste, envoi (y compris par glisser-déposer), téléchargement, édition par double-clic, création de dossier, suppression, suivi du dossier courant du terminal.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [x] Éditer un fichier distant par double-clic (fait le 16/11/2026 : VS Code, Notepad++ ou Bloc-notes ; confirmation à chaque enregistrement, envoi automatique en option ; fusion à trois versions comme git si le fichier a changé sur le serveur, marqueurs de conflit sinon ; choix de l'éditeur pas encore réglable) | Téléchargement, ouverture dans l'éditeur, puis à chaque enregistrement : « Do you want to replace the remote file on … ? » ; éditeur par défaut réglable (« Default text editor program », « Open with… ») | 1 |
| [ ] Renommer un fichier ou un dossier | « Rename » | 2 |
| [ ] Afficher ou masquer les fichiers cachés | `SFTPShowDotFiles` | 2 |
| [ ] Modifier les permissions | Non confirmé par l'analyse côté MobaXterm ; absent de ferroterm | 3 |
| [ ] Comparer deux fichiers | « Compare file with… » (MobaDiff) | 3 |
| [ ] Repli sur SCP si SFTP est indisponible | Repli automatique, désactivable | 3 |

## Connexion SSH

Ferroterm aujourd'hui : clé privée OpenSSH ou mot de passe, enregistrables dans le coffre Windows, vérification `known_hosts`, keepalive de 30 s, délai de connexion de 15 s.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [x] Mots de passe enregistrés (fait le 14/11/2026 : coffre Windows/DPAPI au lieu d'un mot de passe maître ; mots de passe SSH et phrases de passe des clés) | Identifiants mémorisés, protégés par un mot de passe maître | 1 |
| [ ] Agent SSH | Pageant, agent OpenSSH, transfert d'agent (`ForwardAgent`) | 1 |
| [ ] Hôte de rebond | « SSH jump host » : hôte, port et utilisateur de passerelle | 2 |
| [ ] Import des mots de passe et passerelles depuis MobaXterm.ini | L'import actuel ne lit que hôte, port, utilisateur et clé | 2 |
| [ ] Compression | Option par session | 3 |
| [ ] Proxy HTTP | Connexion via `CONNECT` | 3 |

## Macros

Ferroterm aujourd'hui : liste de commandes envoyées ligne par ligne au terminal actif, lancées par double-clic.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [ ] Raccourci clavier par macro | `MacrosHotKeys` | 2 |
| [ ] Enregistrement de la frappe en direct | « Recording terminal macro… » | 2 |
| [ ] Délai entre les lignes | `MacrosDelay` | 3 |
| [x] Dupliquer une macro (déjà présent : clic droit > Dupliquer) | « Duplicate macro » | 3 |
| [ ] Lancer une macro en ligne de commande | `-runmacro` | 3 |

## Supervision du serveur

Ferroterm aujourd'hui : CPU, mémoire, réseau, uptime, utilisateurs, disques, processus les plus gourmands, distribution, avec courbes.

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [ ] Choisir les mesures affichées | Une option par mesure (`MonitorCPU`, `MonitorRAM`…) | 3 |
| [ ] Connexions réseau ouvertes | `MonitorNetstat` | 3 |
| [ ] Partitions NFS et descripteurs de fichiers | `MonitorNfsPartitions`, `MonitoFDs` | 3 |

## Terminal local

Ferroterm aujourd'hui : shells Windows détectés automatiquement (PowerShell, cmd, WSL, Git Bash).

| À reprendre | Ce que fait MobaXterm | Priorité |
|---|---|---|
| [ ] Terminal « popup » | Affiché ou masqué par un raccourci clavier global | 3 |
| [ ] « Ouvrir un terminal ici » | Entrée dans le menu contextuel de l'Explorateur Windows | 3 |

## Déjà au niveau de MobaXterm

- Suivi du dossier courant du terminal par l'explorateur de fichiers.
- Keepalive des connexions.
- Zoom du terminal.
- Sélection = copie et clic droit = coller.

## Proposition de premier lot

1. Recherche dans le terminal.
2. Mots de passe enregistrés et agent SSH.
3. Édition d'un fichier distant par double-clic.
4. Mode multi-exécution.
