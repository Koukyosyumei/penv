#include <string.h>

void *memset(void *dest, int value, size_t size)
{
    unsigned char *bytes = dest;
    while (size-- != 0) {
        *bytes++ = (unsigned char)value;
    }
    return dest;
}

void *memcpy(void *restrict dest, const void *restrict src, size_t size)
{
    unsigned char *out = dest;
    const unsigned char *in = src;
    while (size-- != 0) {
        *out++ = *in++;
    }
    return dest;
}
