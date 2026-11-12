# Projector screensaver (JMGO)

Status: in use on the JMGO since 2026-11-12. **This is not part of Torrbaby.**
It is a standalone open-source app with its clips stored on the projector. This
note lives here only because the JMGO cleanup that led to it happened in a
Torrbaby session. The TCL waits until it has been in use on the JMGO for a
while, and until we have working ADB on the TCL.

## Why

The JMGO's own screensaver (`com.jmgo.os.dream`) showed ads and downloaded
content in the background. It has been removed for user 0 (see "JMGO cleanup"
below). The goal was something like the Apple TV's aerial screensaver: slow,
high-quality video with the time shown, and no network use.

## What is installed

Instead of writing our own app, we use **Aerial Views** 1.8.5
(github.com/theothernt/AerialViews, GPL-3.0), built from source on the Mac:

- The F-Droid flavour, with the Firebase Performance plugin removed so the APK
  contains no Firebase code. Gradle 9.8.0 came from the Tencent mirror, with its
  checksum verified.
- Installed as the debug build `com.neilturner.aerialviews.debug`.
- The Apple, Amazon and community video sources are turned off. Only local
  videos are on, limited to the folder `/Movies/Screensaver/`.
- Settings live in
  `shared_prefs/com.neilturner.aerialviews.debug_preferences.xml` and were set
  with `run-as`. `startup_screensaver_on_launch=true` (opening the app starts
  the screensaver); holding OK opens its settings.

Made the system screensaver over ADB (no root), and the firmware keeps it
across restarts:

```sh
adb shell settings put secure screensaver_components com.neilturner.aerialviews.debug/com.neilturner.aerialviews.ui.screensaver.DreamActivity
adb shell settings put secure screensaver_default_component com.neilturner.aerialviews.debug/com.neilturner.aerialviews.ui.screensaver.DreamActivity
adb shell settings put system screen_off_timeout 600000   # 10 minutes; was 1999999999
```

Start it by hand with `adb shell service call dreams 1`, or by opening Aerial
Views on the projector.

## Clips

The clips are in `/sdcard/Movies/Screensaver/` on the projector, 30 seconds each,
with no audio. 4K sources are HEVC at 18 Mb/s (`hevc_videotoolbox`); 1080p
sources are HEVC at 8 Mb/s. File names are bilingual, for example
`Aurora at night · 夜间极光.mp4`, so the file name works as a caption if
Aerial Views' location overlay is turned on. After pushing new files, make sure MediaStore has indexed them
(Aerial Views finds local videos through it).

Use only footage we are allowed to use. **Not Apple's aerial videos**: they
are Apple's copyrighted content.

- **NASA**: Earth from the ISS ("4K Earth Views Extended Cut", JSC 2021).
  Public domain.
- **Wikimedia Commons**: CC BY and CC BY-SA drone and orbital footage. These
  licences require attribution, which is in the table below. A copy of this
  table is on the projector as `SOURCES.txt` in the clip folder.

| Source on Commons | Author | Licence |
| --- | --- | --- |
| Aerial imagery of fjords and rivers in Arctic Norway | anotherview | CC BY 3.0 |
| Alexander Gerst's Earth timelapses (2017 reissue) | European Space Agency | CC BY-SA 3.0 IGO |
| Drone flyover video of Mount Hiei | Benlisquare | CC BY-SA 4.0 |
| Fall foliage colors in Virginia - 4k Drone view | Sudeep Gowda | CC BY 3.0 |
| Flying the Drone around the Great Wall of China near Jinshan Ridge | Johnny Xmas | CC BY 3.0 |
| Lala Lake, Romania - 4K Aerial | Marian Cepeha | CC BY 3.0 |
| Piz Neir, aerial video 4k | Capricorn4049 | CC BY-SA 4.0 |
| Rheinfall (Rhine Falls) | Harry TheSea | CC BY 3.0 |

Clips cut from these were trimmed and re-encoded, and the Lala Lake clips were
cropped to remove the letterbox and logos. Commons rate-limits large downloads
(HTTP 429): download one file at a time with a descriptive User-Agent, and wait
if it refuses.

## JMGO cleanup so far (2026-11-12, no root)

- `pm disable-user` does **not** survive a restart: the firmware re-enables
  apps at boot.
- `pm uninstall -k --user 0 <app>` **does** survive. Undo it with
  `cmd package install-existing <app>`.
- Removed this way: `fusion.android.tv.demo` (retail demo), `com.jmgo.os.guide`,
  `com.jmgo.os.gamecenter`, `com.jmgo.browser`, `com.jmgo.helpcenter` and
  `com.jmgo.os.dream` (the ad-filled screensaver).
- `com.jmgo.ai.voice` (the voice assistant, a system-uid app kept updated by
  `com.jmgo.update`) is restored by the firmware at boot even after removal.
- The shell cannot disable another app's components, so iQiyi still starts
  itself at boot (`QYBootCompleteReceiver`). That is accepted.
- Kept on purpose: iQiyi, Tencent Video, CIBN, screen mirroring
  (`com.jmgo.holacast.x`), and every projector-hardware app (`hippo`, `spider`,
  keystone, focus, brightness, colour, eye protection, remote).
- The UniFi gateway has a firewall policy, "JMGO projector: block vendor and
  trackers", that blocks the projector from reaching holatek.cn, jmgo.com,
  jmgo.cn, jianguo.tv, umeng.com, umengcloud.com, mob.com, bugly.qq.com,
  sorstek.com and xfyun.cn. It matches the projector by MAC address
  `d2:c6:d0:81:c8:72`, which is a private (randomised) MAC: if the projector's
  Wi-Fi MAC changes, the rule silently stops applying. iQiyi, Tencent Video and
  Baidu were confirmed still working.

## The JMGO home screen

The JMGO launcher (`com.jmgo.launcher`) does not read Android TV's
channels/preview programs. Its carousel is fed by JMGO's servers and by a few
hard-coded partner content providers (CIBN, Mango TV, Coocaa), so a third-party
app such as Torrbaby cannot put cards on it. Showing Torrbaby titles on the home
screen would need a replacement launcher.

A replacement launcher is blocked while `com.jmgo.launcher` is installed: JMGO's
`JmgoWmHook` in `services.jar` sends every Home press to a launcher listed in
`/vendor/etc/launcher.cfg` (`com.jmgo.launcher`, `com.jmgo.arwen`) and ignores
`cmd package set-home-activity`. It falls back to Android's normal choice only
when no listed launcher is installed for the user.

### Torrbaby in JMGO's watch history (观影记录), tested 2026-11-12

The launcher's watch history accepts records only from partner apps, and opens
each record with a link template built into the launcher for that partner. Mango
TV (`com.starcor.mango`) is not installed, so a small bridge app can take its
place:

- The bridge uses the package name `com.starcor.mango` and serves
  `content://com.starcor.mango.playhistory/history`. The launcher starts
  watching that provider when the package is installed, and queries it
  (`updateTime ASC`) whenever the bridge calls `notifyChange`.
- Columns: `videoType` (must be 1), `pid` (int; becomes the launch id),
  `pType`, `serialno` (episode number), `watchTime` and `duration` (ms),
  `updateTime` (ms; only rows newer than the last sync are taken), `pName`,
  `pImage`, `vName`, `vImage`. The card title comes from `vName` and the
  picture from `vImage`. `android.resource://com.starcor.mango/drawable/…`
  images work, so posters do not need to be reachable over the network.
- Choosing the card launches
  `mgtvapp://vod/player?partId=&clipId=<pid>&…` in `com.starcor.mango`; the
  bridge's VIEW activity reads `clipId` and opens Torrbaby. On Android 11 the
  bridge needs a `<queries>` entry for Torrbaby's package to find it.
- The card shows a small Mango TV badge. Installing the real Mango TV app would
  replace the bridge.
- Test APK source: the session scratchpad `mangoshim/` (not yet a project).
