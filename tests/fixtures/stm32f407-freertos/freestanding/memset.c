#include <string.h>

void *memset(void *dest, int value, size_t size)
{
    unsigned char *bytes = dest;
    while (size-- != 0) {
        *bytes++ = (unsigned char)value;
    }
    return dest;
}
