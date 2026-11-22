package com.ferroterm.data

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import android.util.Base64
import android.util.Log
import kotlinx.serialization.json.Json
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Mots de passe mémorisés, chiffrés en AES-GCM par une clé du Keystore Android.
 * La clé ne quitte jamais le Keystore : le fichier `secrets.json` seul est inutilisable
 * sur un autre appareil.
 */
class SecretStore(dir: File) {
    private val file = AtomicFile(File(dir, "secrets.json"))
    private val entries: MutableMap<String, String> = load()

    @Synchronized
    fun get(name: String): String? {
        val stored = entries[name] ?: return null
        return try {
            val (iv, data) = stored.split(':', limit = 2).map { Base64.decode(it, Base64.NO_WRAP) }
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
            cipher.doFinal(data).decodeToString()
        } catch (e: Exception) {
            // Clé du Keystore perdue ou entrée abîmée : le mot de passe sera redemandé.
            Log.w(TAG, "Secret « $name » illisible, supprimé", e)
            remove(name)
            null
        }
    }

    @Synchronized
    fun put(name: String, secret: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val data = cipher.doFinal(secret.encodeToByteArray())
        entries[name] = Base64.encodeToString(cipher.iv, Base64.NO_WRAP) + ":" +
            Base64.encodeToString(data, Base64.NO_WRAP)
        write()
    }

    @Synchronized
    fun remove(name: String) {
        if (entries.remove(name) != null) write()
    }

    @Synchronized
    fun contains(name: String): Boolean = name in entries

    private fun key(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun load(): MutableMap<String, String> {
        if (!file.baseFile.exists()) return mutableMapOf()
        return try {
            Json.decodeFromString<Map<String, String>>(file.readFully().decodeToString()).toMutableMap()
        } catch (e: Exception) {
            Log.e(TAG, "secrets.json illisible", e)
            mutableMapOf()
        }
    }

    private fun write() {
        val out = file.startWrite()
        try {
            out.write(Json.encodeToString(entries.toMap()).encodeToByteArray())
            file.finishWrite(out)
        } catch (e: Exception) {
            file.failWrite(out)
            throw e
        }
    }

    companion object {
        private const val TAG = "SecretStore"
        private const val ANDROID_KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "ferroterm-secrets"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"

        fun passwordName(sessionId: Long) = "password:$sessionId"
        fun passphraseName(keyFile: String) = "passphrase:$keyFile"
    }
}
