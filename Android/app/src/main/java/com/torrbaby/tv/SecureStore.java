package com.torrbaby.tv;

import android.content.*;
import android.security.keystore.*;
import android.util.Base64;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import javax.crypto.*;
import javax.crypto.spec.GCMParameterSpec;
import org.json.JSONObject;

/** Credentials encrypted by a non-exportable Android Keystore key. No backup. */
final class SecureStore {
  private static final String ALIAS = "torrbaby.connection.v1";
  private final SharedPreferences prefs;

  SecureStore(Context c) {
    prefs = c.getSharedPreferences("connection", Context.MODE_PRIVATE);
  }

  private javax.crypto.SecretKey key() throws Exception {
    KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
    ks.load(null);
    if (!ks.containsAlias(ALIAS)) {
      KeyGenerator generator =
          KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
      generator.init(
          new KeyGenParameterSpec.Builder(
                  ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
              .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
              .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
              .build());
      generator.generateKey();
    }
    return (javax.crypto.SecretKey) ks.getKey(ALIAS, null);
  }

  synchronized void save(Api api) throws Exception {
    JSONObject j =
        new JSONObject().put("server", api.base).put("token", api.token).put("client", api.client);
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(Cipher.ENCRYPT_MODE, key());
    String encrypted =
        Base64.encodeToString(
            cipher.doFinal(j.toString().getBytes(StandardCharsets.UTF_8)), Base64.NO_WRAP);
    if (!prefs
        .edit()
        .putString("data", encrypted)
        .putString("iv", Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP))
        .commit()) throw new Exception("Could not save the connection.");
  }

  synchronized Api load() throws Exception {
    String value = prefs.getString("data", "");
    if (value.isEmpty()) return null;
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(
        Cipher.DECRYPT_MODE,
        key(),
        new GCMParameterSpec(128, Base64.decode(prefs.getString("iv", ""), Base64.NO_WRAP)));
    JSONObject j =
        new JSONObject(
            new String(
                cipher.doFinal(Base64.decode(value, Base64.NO_WRAP)), StandardCharsets.UTF_8));
    return new Api(j.getString("server"), j.getString("token"), j.getString("client"));
  }

  void clear() {
    prefs.edit().clear().apply();
  }
}
