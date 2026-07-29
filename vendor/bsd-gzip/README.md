# BSD gzip frontend

The CLI frontend is the BSD-2-Clause gzip implementation from the FreeBSD
source tree (NetBSD-derived), with optional bzip2, xz, zstd, compress and pack
formats disabled for the iOS build. It is compiled as one upstream translation
unit and linked against Apple's public `libz`.

`compat.h` supplies the small Darwin portability definitions (`nitems` and
`st_atim/st_mtim` aliases). `gzip_host.c` converts upstream `exit()` into a
per-invocation return for YourShell's process Host.
