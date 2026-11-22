import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "com.ferroterm"
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "com.ferroterm"
        // Android 15 minimum : un seul comportement à tester pour les services de
        // premier plan, l'affichage bord à bord et le sélecteur de fichiers.
        minSdk = 35
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    // Signature de production : `keystore.properties` (hors Git) à la racine du projet Android.
    val signing = rootProject.file("keystore.properties").takeIf { it.isFile }?.let { file ->
        Properties().apply { file.inputStream().use { load(it) } }
    }
    signingConfigs {
        if (signing != null) {
            create("release") {
                storeFile = file(signing.getProperty("storeFile"))
                storePassword = signing.getProperty("storePassword")
                keyAlias = signing.getProperty("keyAlias")
                keyPassword = signing.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            // R8 : code réduit et optimisé ; règles de conservation dans proguard-rules.pro.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
                ?: error("keystore.properties manquant : impossible de signer la version de production")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources {
            // BouncyCastle : plusieurs jars embarquent les mêmes métadonnées OSGi.
            excludes += setOf(
                "META-INF/versions/9/OSGI-INF/MANIFEST.MF",
                "META-INF/BCKEY.DSA",
                "META-INF/BCKEY.SF",
                "META-INF/DEPENDENCIES",
                "META-INF/LICENSE.md",
                "META-INF/NOTICE.md",
            )
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.11.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    // Les icônes ne suivent plus le BOM (dernière version publiée : 1.7.8).
    implementation("androidx.compose.material:material-icons-core:1.7.8")

    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.navigation:navigation-compose:2.10.2")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.1")

    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.13.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")

    // SSH / SFTP. BouncyCastle complet : celui d'Android est amputé.
    implementation("com.hierynomus:sshj:0.41.2")
    implementation("org.bouncycastle:bcprov-jdk18on:1.87")
    implementation("org.bouncycastle:bcpkix-jdk18on:1.87")

    // FTP / FTPS.
    implementation("commons-net:commons-net:3.13.0")

    // Terminal (libvterm) en composant Compose, Apache 2.0.
    implementation("org.connectbot:termlib:0.3.15")

    testImplementation("junit:junit:4.13.2")
}
