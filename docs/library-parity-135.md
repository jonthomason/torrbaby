# Mobile and Mac library parity

The shared library UI adopts TV's artwork-first cards and detail hero. Continue
Watching cards retain poster dimensions and add a full-width progress line,
episode identification and compact remaining time. Finished titles have a
Recently Watched shelf. Missing posters retain the title as an image fallback;
every card exposes its title to accessibility.

Detail pages put title, metadata, Play/Resume and Download over a shaded backdrop.
Secondary actions remain visible below. Compact screens use smaller outer margins;
Mac and landscape iPad share the same arrangement. Playback preparation overlays
the bottom of the page with Cancel, without moving the content.

Fixed Xcode generator to use platformFilters for the Top Shelf dependency and
embed entry. The previous singular field allowed a TV extension into Mac/iOS
builds. Both signed build135 products pass validation and contain no TV extension.
Existing reviewed EN/ZH strings are reused; localization and diff checks pass.

Visual verification/publication pending: CUA cannot see any app windows, including
Finder (cgWindowNotFound). User asked to unlock. Artifacts under
local-state/library-135-fixed. No physical mobile device installation.
