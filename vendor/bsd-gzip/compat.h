#ifndef YOURSHELL_BSD_GZIP_COMPAT_H
#define YOURSHELL_BSD_GZIP_COMPAT_H

#include <stddef.h>

#define nitems(x) (sizeof(x) / sizeof((x)[0]))
#define st_atim st_atimespec
#define st_mtim st_mtimespec

#endif
