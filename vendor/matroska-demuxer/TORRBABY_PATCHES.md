# Local changes to matroska-demuxer 0.8.1

Upstream: https://github.com/hasenbanck/matroska-demuxer
Source: crates.io 0.8.1. Used under its MIT license; upstream license texts retained.

Fix five metadata field lookups in src/lib.rs:
- Colour MaxCLL and MaxFALL were read from MatrixCoefficients.
- MasteringMetadata red/green/blue Y coordinates were read from X fields.

No other semantic source changes. The source tree and package manifest are
vendored to make builds reproducible; unrelated upstream test media omitted.
Revisit the patch when updating the dependency. Notices generation includes this
path dependency. The HDR metadata fixture checks distinct X/Y and brightness values.
