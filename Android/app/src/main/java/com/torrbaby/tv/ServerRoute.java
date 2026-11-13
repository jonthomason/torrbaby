package com.torrbaby.tv;

/**
 * Where the server was found on the home network during this run of the app. It is never
 * saved: the address comes from DHCP and may change, so each launch finds the server again by
 * its Bonjour service. Until then requests go to a name that never resolves, fail at once, and
 * start that search.
 */
final class ServerRoute {
  static final String UNKNOWN = "http://torrbaby.invalid:8765";
  static volatile String found = "";

  static String current() { return found.isEmpty() ? UNKNOWN : found; }
}
