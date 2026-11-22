# Règles R8 de la version de production.
#
# Ces bibliothèques chargent des classes par leur nom (fournisseurs de sécurité,
# algorithmes, analyseurs de listes FTP) : R8 ne peut pas le voir et les retirerait.
# On les garde entières ; le reste du code (Compose, AndroidX, l'application) est
# réduit et optimisé normalement.

# BouncyCastle : algorithmes enregistrés par nom de classe dans le fournisseur « BC ».
-keep class org.bouncycastle.** { *; }
-dontwarn org.bouncycastle.**

# sshj : fabriques d'algorithmes, clés, SFTP.
-keep class net.schmizz.** { *; }
-keep class com.hierynomus.** { *; }
-dontwarn net.schmizz.**
-dontwarn com.hierynomus.**

# Apache Commons Net : analyseurs de listes FTP chargés par nom.
-keep class org.apache.commons.net.** { *; }
-dontwarn org.apache.commons.net.**

# Journalisation de sshj / termlib : aucune implémentation fournie (journaux ignorés).
-dontwarn org.slf4j.**

# Classes de javax.naming / java.awt référencées par certaines bibliothèques, absentes d'Android.
-dontwarn javax.naming.**
-dontwarn java.awt.**
