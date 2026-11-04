// slimdeer: wasm32 has no C library; nothing slimdeer builds sets errno.
#ifndef SLIMDEER_ERRNO_H_
#define SLIMDEER_ERRNO_H_
extern int errno;
#define EINVAL 22
#define ERANGE 34
#endif
