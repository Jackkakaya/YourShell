#ifndef YOURSHELL_SYS_ENDIAN_H
#define YOURSHELL_SYS_ENDIAN_H

#include <stdint.h>

/*
 * Apple's newer SDKs expose a private compatibility sys/endian.h while older
 * supported Xcode SDKs do not. The imported BSD gzip frontend only needs
 * le32dec, so provide the stable BSD spelling without depending on SDK age.
 */
static inline uint32_t
le32dec(const void *buffer)
{
    const uint8_t *bytes = (const uint8_t *)buffer;
    return ((uint32_t)bytes[0]) |
           ((uint32_t)bytes[1] << 8) |
           ((uint32_t)bytes[2] << 16) |
           ((uint32_t)bytes[3] << 24);
}

#endif
