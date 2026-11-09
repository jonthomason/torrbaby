package com.torrbaby.tv;

import android.content.Intent;

final class Bootstrap {
  static Api from(Intent i) throws Exception {
    String server = i.getStringExtra("test_server");
    return server == null
        ? null
        : new Api(
            server,
            i.getStringExtra("test_token") == null ? "fixture" : i.getStringExtra("test_token"),
            "android-emulator-test");
  }
}
