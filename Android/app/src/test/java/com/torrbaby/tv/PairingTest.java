package com.torrbaby.tv;

import static org.junit.Assert.*;

import java.nio.charset.StandardCharsets;
import org.json.JSONObject;
import org.junit.Test;

/** The test vector in docs/remote-pairing.md (from Diamond's server/pairing.rs, checked with Python). */
public class PairingTest {
  private static byte[] counting(int n) {
    byte[] b = new byte[n];
    for (int i = 0; i < n; i++) b[i] = (byte) i;
    return b;
  }

  @Test
  public void derivesTheDocumentedIdAndKey() throws Exception {
    Pairing p = new Pairing(counting(32));
    assertEquals("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8", Pairing.b64(p.secret));
    assertEquals("7eba2290a9f2307e661a6331d864dfac", p.id);
    assertEquals("f8b6511f9653cedb74f0fcf38c10ae5d89513a82eb9460b42af1cb1d523f45ba", Pairing.hex(p.key));
  }

  @Test
  public void sealsAndOpensTheDocumentedMessage() throws Exception {
    Pairing p = new Pairing(counting(32));
    String sealed = "AAECAwQFBgcICQoLFh39D-g3CmZl7iLvhggJBrTzw2klgw-1rECGYU5IvYKQ3jQ";
    assertEquals(sealed, Pairing.b64(p.seal(counting(12), "{\"v\":1,\"token\":\"t\"}".getBytes(StandardCharsets.UTF_8))));
    // The documented message has no client_id, so it opens but is incomplete as a setup.
    try { p.open(new JSONObject().put("sealed", sealed)); fail(); }
    catch (SecurityException expected) { assertTrue(expected.getMessage().contains("incomplete")); }
  }

  @Test
  public void opensASetupAndRejectsForgeries() throws Exception {
    Pairing p = new Pairing(counting(32));
    String setup = "{\"v\":1,\"client_id\":\"c\",\"token\":\"t\",\"server\":{\"identity\":\"ab\"}}";
    String sealed = Pairing.b64(p.seal(counting(12), setup.getBytes(StandardCharsets.UTF_8)));
    assertEquals("c", p.open(new JSONObject().put("sealed", sealed)).getString("client_id"));
    assertNull(p.open(new JSONObject().put("waiting", true)));
    // Sealed for another code (another id and key) does not open.
    Pairing other = Pairing.fresh();
    String foreign = Pairing.b64(other.seal(counting(12), setup.getBytes(StandardCharsets.UTF_8)));
    try { p.open(new JSONObject().put("sealed", foreign)); fail(); } catch (javax.crypto.AEADBadTagException expected) { }
    // A flipped byte does not open either.
    char[] c = sealed.toCharArray(); c[20] = c[20] == 'A' ? 'B' : 'A';
    try { p.open(new JSONObject().put("sealed", new String(c))); fail(); } catch (javax.crypto.AEADBadTagException expected) { }
  }

  @Test
  public void codeCarriesTheSecretNotTheKey() throws Exception {
    Pairing p = new Pairing(counting(32));
    JSONObject code = new JSONObject(p.code("JMGO"));
    assertEquals("torrbaby", code.getString("app"));
    assertEquals("join", code.getString("kind"));
    assertEquals(1, code.getInt("v"));
    assertEquals("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8", code.getString("secret"));
    assertEquals("android-tv", code.getString("platform"));
  }
}
