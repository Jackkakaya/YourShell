# bsdtar

This directory contains the command-line frontend from libarchive 3.8.8.
The unmodified upstream files are `tar/*`, `libarchive_fe/*`, the `archive*.h`
headers, `archive_parse_date.c`, `config.h`, and `COPYING`. `bsdtar_host.c` is
YourShell's small in-process lifecycle wrapper.

Upstream release:
https://github.com/libarchive/libarchive/releases/tag/v3.8.8

Release archive SHA-256:
`3873a88801da067d0528a989af06877710529d50ee8fe6f3970cbb4302efb918`

Only the CLI frontend is compiled. Both macOS and iOS provide public
`libarchive`, so archive formats, compression filters, and security fixes come
from the platform library rather than a second vendored copy.
`archive_parse_date.c` is the sole exception: it is a public-domain helper
introduced after the libarchive version currently shipped by Apple.
