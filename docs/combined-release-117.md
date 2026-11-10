# Combined release 117 / Android 39

Apple build 117 includes the unified processing queue labels and shared mobile sidebar with blue icons, small selection corners, centered branding/status, and blue EN/中文 controls. Android 0.3.13 build 39 includes the queue translations and blue language selector.

Published iPhone/iPad, Mac ZIP, Android APK, and tvOS simulator ZIP to the Miyu Install Torrbaby page. All four HTTPS artifacts were downloaded and SHA-256 checked against publication metadata. iOS and Mac retain com.jonthomason.torrbaby.ios and com.jonthomason.torrbaby.mac. Android retains its existing signing certificate. Mac installed at ~/Applications/Torrbaby.app and one process confirmed. No physical iPhone/iPad installations performed. tvOS simulator installed; no paired physical Apple TV deployment.

Validation: iOS, macOS, tvOS simulator, Android release builds passed; iOS simulator build passed; 581 EN/ZH localization entries checked; three processing stage/sign-off/recovery tests passed; git diff whitespace check passed. Existing Diamond unified processing deployment confirmed with database status. No Chinese source requests made.

Limitation: native desktop automation returned cgWindowNotFound for both Torrbaby and Device Hub, so this release has not received a fresh visual EN/ZH pass. Build success is not visual verification. The simulator builds are available for that follow-up.
