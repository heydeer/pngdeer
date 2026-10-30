// pngdeer: wasm32 has no C library; nothing pngdeer builds sets errno.
#ifndef PNGDEER_ERRNO_H_
#define PNGDEER_ERRNO_H_
extern int errno;
#define EINVAL 22
#define ERANGE 34
#endif
