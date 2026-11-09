package com.torrbaby.tv;

import static org.junit.Assert.*;

import org.junit.Test;

public class ApiTest {
  @Test
  public void rejectsCredentialBearingAndNonServerAddresses() throws Exception {
    for (String s :
        new String[] {
          "file:///etc/passwd",
          "http://u:secret@host",
          "http://host/path",
          "http://host?token=secret",
          "javascript:alert(1)",
          "http://host#fragment"
        }) {
      try {
        Api.validateBase(s);
        fail(s);
      } catch (Exception expected) {
      }
    }
    assertEquals("http://host:8765", Api.validateBase(" http://host:8765/ "));
  }

  @Test
  public void signedMediaCannotEscapeServer() throws Exception {
    Api a = new Api("http://host:8765", "token", "client");
    assertEquals("http://host:8765/v1/stream/a/0?sig=abc", a.url("/v1/stream/a/0?sig=abc"));
    for (String path :
        new String[] {
          "http://evil/v1/file",
          "https://host:8765/v1/file",
          "//evil/v1/file",
          "/elsewhere",
          "/v1/../../secret",
          "http://user@host:8765/v1/file"
        }) {
      try {
        a.url(path);
        fail(path);
      } catch (Exception expected) {
      }
    }
  }

  @Test
  public void titleIdsKeepLiteralColonsExpectedByServer() {
    assertEquals("tmdb:movie:348", Api.part("tmdb:movie:348"));
    assertEquals("a%2Fb%20c", Api.part("a/b c"));
  }
}
