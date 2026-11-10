# Apple bundle identifiers — build 114

macOS: `com.jonthomason.torrbaby.mac`
iPhone/iPad: `com.jonthomason.torrbaby.ios`

Both the generated Xcode project and its generator use these identifiers. macOS copies missing preferences from the previous org.torrbaby.mac / org.torrbaby.app domains before constructing Store, including the existing device identity and subtitle choices. Existing Keychain service identifiers remain stable. The process lock remains shared across old and new bundle identifiers.

Both signed release builds passed; embedded bundle identifiers verified. Mac installed at ~/Applications/Torrbaby.app; mobile 114 published to Miyu Install with six registered devices in the profile. iOS treats this as a separate installation: pairing and offline downloads do not automatically migrate from the previous sandbox. The previous mobile app is retained for the user to remove when ready. tvOS identifier is unchanged.
