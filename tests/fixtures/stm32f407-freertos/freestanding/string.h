#ifndef FIXTURE_STRING_H
#define FIXTURE_STRING_H
#include <stddef.h>
void *memset(void *dest, int value, size_t size);
void *memcpy(void *restrict dest, const void *restrict src, size_t size);
#endif
