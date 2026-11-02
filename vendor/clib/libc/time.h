// pngdeer: wasm32 has no C library; libaom includes this header without calling into it (its
// timers are compiled out: CONFIG_OS_SUPPORT is 0).
#ifndef PNGDEER_TIME_H_
#define PNGDEER_TIME_H_
#endif
