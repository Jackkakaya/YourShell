# bsdunzip

Official libarchive 3.8.8 `bsdunzip` frontend, BSD-2-Clause licensed. The
frontend is compiled in-process and linked against Apple's public libarchive;
YourShell only adds `bsdunzip_host.c` to turn `exit()` into an invocation return.
