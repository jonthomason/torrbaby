# Pending next release: premature watched completion

Do not build until user requests the accumulated UI release.

The daemon previously marked playback watched at >=90% with <=600 seconds remaining. Bone Lake on profile-jon was observed at position 5138.563177778 / duration 5667.412, watched=1. This is 8m49s before EOF. Continue Watching correctly excludes watched movies and sorts remaining titles by last_played descending; hero selection is independent.

Source fix: server/profiles.rs completion requires actual duration; explicit reached_end and Mark as Watched remain supported. Apple offline DownloadStorage uses the same rule. Next Episode remains an explicit completion action; its availability threshold is not itself a watched flag. This shared daemon rule covers Android and all Apple streaming clients.

After deploying the corrected daemon, restore Bone Lake's profile-jon watched flag without altering position or last_played (media id 34a6d501430139cff5abbcdbc465d178db36aecb:0). Re-read first: only repair if still below duration and not subsequently explicitly completed; do not blindly reset after the user finishes the movie. Check whether any hidden-title flag also applies. Preserve all other histories. Old offline clients retain the old heuristic until updated.

No builds or deployment performed for this change. Existing completion unit test updated but not run during the requested no-build interval.
