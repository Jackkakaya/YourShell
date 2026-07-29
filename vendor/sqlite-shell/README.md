# SQLite command-line shell

`shell.c` and `sqlite3.h` originate from the official SQLite 3.46.0
amalgamation:

https://www.sqlite.org/2024/sqlite-amalgamation-3460000.zip

SHA-256:

- archive: `712a7d09d2a22652fb06a49af516e051979a3984adb067da86760e60ed51a7f5`
- original shell.c: `8bf29000bbbe93a4cff05c07eb210536f7607b9dc8d007ef08b8c4b12368df01`
- sqlite3.h: `d088aa96aa70db50f02acc5c86eca61a5d17556e4c363b9c06079239bf7f87b1`

SQLite is in the public domain. `sqlite3.c` is deliberately not copied: the
same 3.46.0 engine is already compiled and linked by `libsqlite3-sys` through
the `rusqlite` dependency.

YourShell-specific process-host behavior lives primarily in `sqlite_host.c`.
Build flags rename upstream `main`, `exit`, and `atexit`. The small marked
patch in `shell.c` routes `system()` to the Host and exposes a repeat-invocation
state reset; it does not change the upstream CLI parser. `system()` returns
`ENOSYS`, matching iOS's no-process-spawning constraint.
